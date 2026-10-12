import DiskfreeCore
import Foundation
import Observation

enum SortMode: UInt8, CaseIterable, Identifiable {
    case size = 0, name = 1, files = 2
    var id: UInt8 { rawValue }
    var label: String { ["Size", "Name", "Files"][Int(rawValue)] }
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
    /// Bumped after every explorer change so views re-read rows and nodes.
    private(set) var revision = 0

    var sort: SortMode = .size {
        didSet { if let ex { df_set_sort(ex, sort.rawValue); changed() } }
    }
    var filter = "" {
        didSet { if let ex { df_set_filter(ex, filter); changed() } }
    }

    @ObservationIgnored private var job: OpaquePointer?
    @ObservationIgnored private(set) var ex: OpaquePointer?

    var hasTree: Bool { _ = revision; return ex != nil }

    func scan() {
        guard job == nil else { return }
        job = df_scan_start(rootPath)
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
        guard let job else { return }
        let cancelled = df_scan_cancelled(job)
        let fresh = df_scan_finish(job, sort.rawValue)
        self.job = nil
        if let ex { df_explorer_free(ex) }
        ex = fresh
        if !filter.isEmpty { df_set_filter(fresh, filter) }
        scanning = false
        status = cancelled ? "Scan cancelled — showing partial results." : ""
        changed()
    }

    private func changed() { revision += 1 }

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
