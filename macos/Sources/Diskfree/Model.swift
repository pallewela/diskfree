import AppKit
import CoreServices
import DiskfreeCore
import Foundation
import Observation

enum SortMode: UInt8, CaseIterable, Identifiable {
    case size = 0, name = 1, files = 2
    var id: UInt8 { rawValue }
    var label: String { ["Size", "Name", "Files"][Int(rawValue)] }
}

struct FileType: Identifiable {
    let ext: String
    let bytes: UInt64
    let files: UInt64
    var id: String { ext }
}

struct Volume: Identifiable {
    let path: String
    let name: String
    let free: Int64
    var id: String { path }

    static func mounted() -> [Volume] {
        let keys: [URLResourceKey] = [.volumeNameKey, .volumeAvailableCapacityForImportantUsageKey]
        let urls = FileManager.default.mountedVolumeURLs(includingResourceValuesForKeys: keys, options: [.skipHiddenVolumes]) ?? []
        return urls.map { url in
            let v = try? url.resourceValues(forKeys: Set(keys))
            return Volume(path: url.path, name: v?.volumeName ?? url.path, free: v?.volumeAvailableCapacityForImportantUsage ?? 0)
        }
    }
}

/// One visible tree row, copied out of the Rust explorer.
struct Row: Identifiable {
    let id: UInt32
    let depth: Int
    let node: DfNode
    let name: String
    let hint: String?
}

func formatBytes<T: BinaryInteger>(_ b: T) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .file)
}

/// Owns the Rust explorer and scan handles. All calls happen on the main actor.
@MainActor @Observable
final class Model {
    var rootPath = "/"
    var scanning = false
    var scannedFiles: UInt64 = 0
    var scannedBytes: UInt64 = 0
    var status = ""
    var dimDays = 0
    /// Visible rows, rebuilt after every explorer change.
    private(set) var rows: [Row] = []
    var selection = Set<UInt32>()
    var showInsights = true
    /// Insights, refreshed after every explorer change.
    private(set) var candidates: [UInt32] = []
    private(set) var largest: [UInt32] = []
    private(set) var types: [FileType] = []
    /// Row id the tree should scroll to (set by `jump`).
    var scrollTarget: UInt32?
    /// Top-level ids awaiting the Move to Trash confirmation.
    var pendingTrash: [UInt32] = []
    /// Bumped after every explorer change so views re-read nodes.
    private(set) var revision = 0

    var sort: SortMode = .size {
        didSet { if let ex { df_set_sort(ex, sort.rawValue); changed() } }
    }
    /// Follow changes on disk with FSEvents (View ▸ Live Updates).
    var liveUpdates = true {
        didSet {
            guard !watchedRoot.isEmpty else { return }
            watch(watchedRoot)
            if liveUpdates, ex != nil { status = "Live updates on. Rescan to pick up changes made while they were off." }
        }
    }
    var filter = "" {
        didSet { if let ex { df_set_filter(ex, filter); changed() } }
    }

    @ObservationIgnored private var job: OpaquePointer?
    @ObservationIgnored private var target: UInt32? // job is a rescan of this node
    @ObservationIgnored private var rescanQueue: [UInt32] = []
    @ObservationIgnored private(set) var ex: OpaquePointer?
    @ObservationIgnored private var stream: FSEventStreamRef?
    @ObservationIgnored private var watchedRoot = ""
    /// Folders FSEvents reported as changed -> whether they need a full rescan (events were dropped).
    @ObservationIgnored private var dirty: [String: Bool] = [:]

    var hasTree: Bool { _ = revision; return ex != nil }

    func scan() {
        guard job == nil else { return }
        job = df_scan_start(rootPath)
        watch(rootPath)
        scanning = true
        status = ""
        Task { await poll() }
    }

    func cancel() {
        if let job { df_scan_cancel(job) }
    }

    private func poll() async {
        while let job, !df_scan_done(job) {
            var files: UInt64 = 0, bytes: UInt64 = 0
            df_scan_progress(job, &files, &bytes)
            (scannedFiles, scannedBytes) = (files, bytes)
            try? await Task.sleep(for: .milliseconds(100))
        }
        adopt()
    }

    /// Scan synchronously (for `--cli`).
    func load(_ path: String) {
        rootPath = path
        job = df_scan_start(path)
        watch(path)
        adopt()
    }

    /// Take the finished (or cancelled) job's tree as the new explorer.
    private func adopt() {
        guard let job else { return }
        if let id = target, let ex {
            // A cancelled rescan is discarded by the core; stop the queue too.
            let cancelled = df_scan_cancelled(job)
            df_scan_finish_graft(job, ex, id)
            (self.job, target) = (nil, nil)
            if cancelled {
                rescanQueue = []
                status = "Rescan cancelled."
            }
            if !startNextRescan() { scanning = false }
            changed()
            applyChanges()
            return
        }
        let cancelled = df_scan_cancelled(job)
        let fresh = df_scan_finish(job, sort.rawValue)
        self.job = nil
        if let ex { df_explorer_free(ex) }
        ex = fresh
        if !filter.isEmpty { df_set_filter(fresh, filter) }
        scanning = false
        status = cancelled ? "Scan cancelled — showing partial results." : ""
        changed()
        applyChanges()
    }

    // MARK: - Live updates

    /// Watch `root` with FSEvents from now on, so changes made during the scan are caught too.
    /// Only stops the current stream while live updates are off.
    private func watch(_ root: String) {
        if let stream {
            FSEventStreamStop(stream)
            FSEventStreamInvalidate(stream)
            FSEventStreamRelease(stream)
            self.stream = nil
        }
        (dirty, watchedRoot) = ([:], root)
        guard liveUpdates else { return }
        // Unretained: the model lives as long as the app.
        var ctx = FSEventStreamContext(version: 0, info: Unmanaged.passUnretained(self).toOpaque(),
                                       retain: nil, release: nil, copyDescription: nil)
        let callback: FSEventStreamCallback = { _, info, count, paths, flags, _ in
            let model = Unmanaged<Model>.fromOpaque(info!).takeUnretainedValue()
            let paths = Unmanaged<CFArray>.fromOpaque(paths).takeUnretainedValue() as! [String]
            let deep = FSEventStreamEventFlags(kFSEventStreamEventFlagMustScanSubDirs)
            MainActor.assumeIsolated {
                for i in 0..<count {
                    model.dirty[paths[i], default: false] = model.dirty[paths[i]] == true || flags[i] & deep != 0
                }
                model.applyChanges()
            }
        }
        stream = FSEventStreamCreate(nil, callback, &ctx, [root] as CFArray,
                                     FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 2.0,
                                     FSEventStreamCreateFlags(kFSEventStreamCreateFlagUseCFTypes))
        if let stream {
            FSEventStreamSetDispatchQueue(stream, .main)
            FSEventStreamStart(stream)
        }
    }

    /// Fold changed folders into the tree: re-read them in place, rescan new folders in the
    /// background. Waits while a scan or rescan runs; `adopt` calls it again afterwards.
    private func applyChanges() {
        guard job == nil, let ex, !dirty.isEmpty else { return }
        var shallow: [UInt32] = [], deep = Set<UInt32>()
        for (path, mustScan) in dirty {
            let id = df_find(ex, path)
            guard id != UInt32.max, node(id)?.isDir == true else { continue }
            if mustScan { deep.insert(id) } else { shallow.append(id) }
        }
        dirty = [:]
        var fresh = [UInt32](repeating: 0, count: 256)
        let n = df_refresh_dirs(ex, shallow, shallow.count, &fresh, fresh.count)
        if n > fresh.count { deep.formUnion(shallow) } else { deep.formUnion(fresh.prefix(n)) }
        changed()
        rescanQueue = topLevel(deep)
        _ = startNextRescan()
    }

    private func changed() {
        rows = []
        if let ex {
            let count = df_row_count(ex)
            rows.reserveCapacity(count)
            for i in 0..<count {
                var id: UInt32 = 0, depth: UInt16 = 0
                var n = DfNode()
                guard df_row(ex, i, &id, &depth), df_node(ex, id, &n) else { continue }
                rows.append(Row(id: id, depth: Int(depth), node: n, name: n.nameString, hint: n.hintString))
            }
            selection = selection.filter { df_is_live(ex, $0) }
            candidates = ids { df_candidates(ex, $0, $1) }
            largest = ids { df_largest(ex, $0, $1) }
            types = (0..<df_type_count(ex)).compactMap { i in
                var t = DfType()
                guard df_type(ex, i, &t) else { return nil }
                let ext = String(decoding: UnsafeBufferPointer(start: t.ext, count: t.ext_len), as: UTF8.self)
                return FileType(ext: ext, bytes: t.bytes, files: t.files)
            }
        } else {
            (selection, candidates, largest, types) = ([], [], [], [])
        }
        revision += 1
    }

    /// Call a "write up to cap ids, return the full count" function twice to get all ids.
    private func ids(_ f: (UnsafeMutablePointer<UInt32>?, Int) -> Int) -> [UInt32] {
        let n = f(nil, 0)
        var out = [UInt32](repeating: 0, count: n)
        _ = out.withUnsafeMutableBufferPointer { f($0.baseAddress, n) }
        return out
    }

    /// Expand down to `id` (clearing the filter), select it and scroll to it.
    func jump(_ id: UInt32) {
        guard let ex else { return }
        _ = df_reveal(ex, id)
        if filter.isEmpty { changed() } else { filter = "" } // didSet re-applies and refreshes
        selection = [id]
        scrollTarget = id
    }

    // MARK: - Actions

    func rescan(_ ids: Set<UInt32>) {
        guard job == nil else { return }
        rescanQueue = topLevel(ids).filter { node($0)?.isDir == true }
        status = ""
        _ = startNextRescan()
    }

    private func startNextRescan() -> Bool {
        guard let ex, !rescanQueue.isEmpty else { return false }
        let id = rescanQueue.removeFirst()
        target = id
        job = df_rescan_start(ex, id)
        scanning = true
        Task { await poll() }
        return true
    }

    func askTrash(_ ids: Set<UInt32>) {
        pendingTrash = topLevel(ids).filter { $0 != 0 }
    }

    func trashPending() {
        guard let ex else { return }
        var gone: [UInt32] = [], bytes: UInt64 = 0, errors: [String] = []
        for id in pendingTrash {
            let url = URL(fileURLWithPath: path(id))
            do {
                try FileManager.default.trashItem(at: url, resultingItemURL: nil)
                gone.append(id)
                bytes += node(id)?.size ?? 0
            } catch {
                errors.append("\(url.lastPathComponent): \(error.localizedDescription)")
            }
        }
        pendingTrash = []
        if !gone.isEmpty { df_remove(ex, gone, gone.count) }
        status = "Moved \(gone.count) item(s), \(formatBytes(bytes)), to Trash."
        if let first = errors.first { status += " \(errors.count) failed — \(first)" }
        changed()
    }

    func reveal(_ ids: Set<UInt32>) {
        NSWorkspace.shared.activateFileViewerSelecting(topLevel(ids).map { URL(fileURLWithPath: path($0)) })
    }

    func copyPaths(_ ids: Set<UInt32>) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(topLevel(ids).map(path).joined(separator: "\n"), forType: .string)
    }

    func toggle(_ id: UInt32) {
        guard let ex, let n = node(id), n.child_count > 0 else { return }
        df_toggle(ex, id, !n.expanded)
        changed()
    }

    func setExpanded(_ ids: Set<UInt32>, _ open: Bool) {
        guard let ex else { return }
        for id in ids { if let n = node(id), n.child_count > 0 { df_toggle(ex, id, open) } }
        changed()
    }

    /// Selection minus anything inside another selected item, so nothing counts twice.
    func topLevel(_ ids: Set<UInt32>) -> [UInt32] {
        guard let ex, !ids.isEmpty else { return [] }
        let input = Array(ids)
        var out = [UInt32](repeating: 0, count: input.count)
        let n = df_top_level(ex, input, input.count, &out, out.count)
        return Array(out.prefix(n))
    }

    func selectionSize() -> (count: Int, bytes: UInt64) {
        let top = topLevel(selection)
        return (top.count, top.reduce(0) { $0 + (node($1)?.size ?? 0) })
    }

    // MARK: - Reading the tree

    func node(_ id: UInt32) -> DfNode? {
        guard let ex else { return nil }
        var n = DfNode()
        return df_node(ex, id, &n) ? n : nil
    }

    func path(_ id: UInt32) -> String {
        guard let ex else { return "" }
        let len = df_path(ex, id, nil, 0)
        var buf = [UInt8](repeating: 0, count: len)
        _ = df_path(ex, id, &buf, len)
        return String(decoding: buf, as: UTF8.self)
    }

    /// Total, used and free bytes of the volume holding the scan root.
    func diskUsage() -> (total: Int64, free: Int64)? {
        _ = revision
        guard ex != nil else { return nil }
        let keys: Set<URLResourceKey> = [.volumeTotalCapacityKey, .volumeAvailableCapacityForImportantUsageKey]
        guard let v = try? URL(fileURLWithPath: path(0)).resourceValues(forKeys: keys),
              let total = v.volumeTotalCapacity, let free = v.volumeAvailableCapacityForImportantUsage
        else { return nil }
        return (Int64(total), free)
    }
}

extension DfNode {
    var nameString: String { String(decoding: UnsafeBufferPointer(start: name, count: name_len), as: UTF8.self) }
    var hintString: String? {
        hint == nil ? nil : String(decoding: UnsafeBufferPointer(start: hint, count: hint_len), as: UTF8.self)
    }
    var isDir: Bool { kind == UInt8(DF_KIND_DIR) || kind == UInt8(DF_KIND_DENIED) }
}
