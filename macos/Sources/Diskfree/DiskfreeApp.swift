import AppKit
import DiskfreeCore
import SwiftUI

@main
struct DiskfreeApp: App {
    @State private var model = Model()

    init() {
        let args = CommandLine.arguments
        if args.count > 2, args[1] == "--cli" {
            cli(args[2])
            exit(0)
        }
        if args.count > 1, args[1] == "--selftest" {
            selftest()
            exit(0)
        }
        // Running as a bare SwiftPM executable: become a regular, frontmost app.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async { NSApp.activate() }
    }

    var body: some Scene {
        WindowGroup("diskfree") {
            ContentView(model: model)
                .frame(minWidth: 760, minHeight: 480)
                .onAppear {
                    // `Diskfree <path>` scans on launch, like the egui app.
                    let args = CommandLine.arguments
                    if args.count > 1 {
                        model.rootPath = args[1]
                        model.scan()
                    }
                }
        }
        .defaultSize(width: 1100, height: 720)
        .commands {
            CommandGroup(after: .sidebar) {
                Toggle("Live Updates", isOn: $model.liveUpdates)
            }
        }
    }
}

/// Headless check of the Rust core through the view model: prints the root and its 20 largest
/// children, and sanity-checks expand/collapse and selection dedup.
@MainActor func cli(_ path: String) {
    let model = Model()
    model.load(path)
    model.toggle(0)
    for row in model.rows.prefix(21) {
        let size = formatBytes(row.node.size).padding(toLength: 10, withPad: " ", startingAt: 0)
        print("\(size)  \(row.name)\(row.hint.map { "  [\($0)]" } ?? "")")
    }
    if let first = model.rows.dropFirst().first {
        precondition(model.topLevel([0, first.id]) == [0], "topLevel must drop nested ids")
    }
    model.toggle(0)
    precondition(model.rows.count == 1, "collapsing the root leaves one row")
}

/// End-to-end check of the actions on a temp folder: trash (real Trash) and rescan.
@MainActor func selftest() {
    let fm = FileManager.default
    let dir = fm.temporaryDirectory.appendingPathComponent("diskfree-selftest-\(getpid())")
    try? fm.removeItem(at: dir)
    try! fm.createDirectory(at: dir.appendingPathComponent("junk"), withIntermediateDirectories: true)
    try! Data(count: 1_000_000).write(to: dir.appendingPathComponent("junk/big.bin"))
    try! Data(count: 100_000).write(to: dir.appendingPathComponent("keep.bin"))

    let model = Model()
    model.load(dir.path)
    model.toggle(0)
    let junk = model.rows.first { $0.name == "junk" }!.id
    let keepSize = model.rows.first { $0.name == "keep.bin" }!.node.size
    model.selection = [junk, 0]
    model.askTrash(model.selection)
    precondition(model.pendingTrash.isEmpty, "root (and anything under a selected root) is never trashed")
    model.askTrash([junk])
    model.trashPending()
    let trashed = model.status
    precondition(!fm.fileExists(atPath: dir.appendingPathComponent("junk").path), "junk moved to Trash")
    precondition(model.node(0)!.size == keepSize, "root total drops by the trashed size")
    precondition(model.rows.count == 2, "root + keep.bin")

    try! Data(count: 300_000).write(to: dir.appendingPathComponent("new.bin"))
    model.rescan([0])
    while model.scanning { RunLoop.main.run(until: Date().addingTimeInterval(0.05)) }
    precondition(model.rows.contains { $0.name == "new.bin" }, "rescan picks up new files")
    precondition(model.node(0)!.files == 2)
    precondition(model.largest.first.flatMap(model.node)?.nameString == "new.bin", "largest files refreshed")
    precondition(model.types.first?.ext == "bin")
    model.toggle(0)
    let newBin = model.largest.first!
    model.jump(newBin)
    precondition(model.rows.contains { $0.id == newBin } && model.selection == [newBin], "jump expands and selects")

    // Live updates: a new file is picked up in place, a new folder by a background rescan.
    func waitFor(_ what: String, _ ok: () -> Bool) {
        let end = Date().addingTimeInterval(15)
        while !ok() {
            precondition(Date() < end, "timed out waiting: \(what)")
            RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        }
    }
    let before = model.node(0)!.size
    try! Data(count: 1_000_000).write(to: dir.appendingPathComponent("live.bin"))
    try! fm.createDirectory(at: dir.appendingPathComponent("livedir"), withIntermediateDirectories: true)
    try! Data(count: 200_000).write(to: dir.appendingPathComponent("livedir/x.bin"))
    waitFor("live file and folder") {
        !model.scanning && model.rows.contains { $0.name == "live.bin" }
            && model.rows.contains { $0.name == "livedir" && $0.node.size > 0 }
    }
    let liveDir = model.rows.first { $0.name == "livedir" }!.node.size
    precondition(model.node(0)!.size > before + liveDir, "root total grows")
    try! fm.removeItem(at: dir.appendingPathComponent("live.bin"))
    waitFor("deleted file drops out") { model.node(0)!.size == before + liveDir }

    model.liveUpdates = false
    try! Data(count: 500_000).write(to: dir.appendingPathComponent("ignored.bin"))
    RunLoop.main.run(until: Date().addingTimeInterval(4))
    precondition(!model.rows.contains { $0.name == "ignored.bin" }, "no updates while live updates are off")
    try? fm.removeItem(at: dir)
    print("selftest ok — \(trashed)")
}

struct ContentView: View {
    let model: Model

    var body: some View {
        @Bindable var model = model
        ZStack(alignment: .bottom) {
            if model.hasTree {
                TreeTable(model: model)
            } else {
                ContentUnavailableView("Pick a volume or a folder", systemImage: "internaldrive",
                                       description: Text("Then press Scan."))
            }
            if model.scanning {
                ScanProgress(model: model)
            } else if model.hasTree {
                InfoBar(model: model)
            }
        }
        .toolbar { Toolbar(model: model) }
        .searchable(text: $model.filter, placement: .toolbar, prompt: "Filter names")
        .inspector(isPresented: $model.showInsights) {
            InsightsView(model: model).inspectorColumnWidth(min: 280, ideal: 360, max: 600)
        }
    }
}

struct TreeTable: View {
    let model: Model

    var body: some View {
        @Bindable var model = model
        let now = UInt64(Date().timeIntervalSince1970)
        let recent = model.dimDays == 0 ? UInt64.max : now - UInt64(model.dimDays) * 86_400
        ScrollViewReader { proxy in
            Table(model.rows, selection: $model.selection) {
                TableColumn("Name") { row in
                    NameCell(model: model, row: row, dimmed: row.node.mtime > recent)
                }
                .width(min: 260, ideal: 480)
                TableColumn("Size") { row in
                    Text(formatBytes(row.node.size)).monospacedDigit()
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(min: 70, ideal: 90)
                TableColumn("%") { row in
                    let p = row.node.parent_size == 0 ? 0 : Double(row.node.size) / Double(row.node.parent_size)
                    // Drawn, not ProgressView: Table reuses cells while scrolling, and a reused
                    // ProgressView animates from the previous row's value to the new one.
                    Capsule().fill(.quaternary)
                        .overlay(alignment: .leading) {
                            GeometryReader { g in Capsule().fill(.tint).frame(width: g.size.width * min(p, 1)) }
                        }
                        .frame(height: 5)
                }
                .width(min: 60, ideal: 90)
                TableColumn("Files") { row in
                    Text(row.node.files, format: .number).monospacedDigit().foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .trailing)
                }
                .width(min: 50, ideal: 80)
                TableColumn("Age") { row in
                    Text(row.node.mtime == 0 ? "" : age(now &- row.node.mtime)).foregroundStyle(.secondary)
                }
                .width(min: 40, ideal: 55)
            }
            .contextMenu(forSelectionType: UInt32.self) { ids in
                if !ids.isEmpty { RowMenu(model: model, ids: ids) }
            } primaryAction: { ids in
                // Double-click / Return toggles folders.
                for id in ids { model.toggle(id) }
            }
            .onKeyPress(.rightArrow) { model.setExpanded(model.selection, true); return .handled }
            .onKeyPress(.leftArrow) { model.setExpanded(model.selection, false); return .handled }
            .onDeleteCommand { model.askTrash(model.selection) }
            .onChange(of: model.scrollTarget) { _, id in
                if let id { proxy.scrollTo(id, anchor: .center) }
                model.scrollTarget = nil
            }
            .confirmationDialog(
                trashTitle,
                isPresented: Binding(get: { !model.pendingTrash.isEmpty }, set: { if !$0 { model.pendingTrash = [] } }),
                titleVisibility: .visible
            ) {
                Button("Move to Trash", role: .destructive) { model.trashPending() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text(trashMessage)
            }
        }
    }

    private var trashTitle: String {
        let n = model.pendingTrash.count
        return n == 1 ? "Move “\(model.node(model.pendingTrash[0])?.nameString ?? "")” to Trash?" : "Move \(n) items to Trash?"
    }

    private var trashMessage: String {
        let ids = model.pendingTrash
        let nodes = ids.compactMap(model.node)
        let size = nodes.reduce(0) { $0 + $1.size }, files = nodes.reduce(0) { $0 + $1.files }
        var lines = ids.prefix(10).map(model.path)
        if ids.count > 10 { lines.append("…and \(ids.count - 10) more") }
        lines.append("\(formatBytes(size)) in \(files) files")
        return lines.joined(separator: "\n")
    }

    private func age(_ secs: UInt64) -> String {
        switch secs / 86_400 {
        case 0: "today"
        case let d where d < 60: "\(d)d"
        case let d where d < 730: "\(d / 30)mo"
        case let d: "\(d / 365)y"
        }
    }
}

struct RowMenu: View {
    let model: Model
    let ids: Set<UInt32>

    var body: some View {
        let top = model.topLevel(ids)
        let many = top.count > 1 ? " (\(top.count) items)" : ""
        let dirs = top.filter { model.node($0)?.isDir == true }
        Button("Reveal in Finder\(many)", systemImage: "folder") { model.reveal(ids) }
        Button("Copy Path\(top.count > 1 ? "s" : "")", systemImage: "doc.on.doc") { model.copyPaths(ids) }
        Button(dirs.count > 1 ? "Rescan \(dirs.count) Folders" : "Rescan", systemImage: "arrow.clockwise") {
            model.rescan(ids)
        }
        .disabled(dirs.isEmpty || model.scanning)
        Divider()
        Button(top.count > 1 ? "Move \(top.count) Items to Trash…" : "Move to Trash…", systemImage: "trash", role: .destructive) {
            model.askTrash(ids)
        }
        .disabled(top.allSatisfy { $0 == 0 })
    }
}

struct NameCell: View {
    let model: Model
    let row: Row
    let dimmed: Bool

    var body: some View {
        let n = row.node
        HStack(spacing: 4) {
            Color.clear.frame(width: CGFloat(row.depth) * 14, height: 1)
            Button {
                model.toggle(row.id)
            } label: {
                Image(systemName: "chevron.right")
                    .rotationEffect(.degrees(n.expanded ? 90 : 0))
                    .font(.caption.weight(.semibold))
                    .frame(width: 14)
            }
            .buttonStyle(.plain)
            .opacity(n.child_count > 0 ? 1 : 0)
            .disabled(n.child_count == 0)
            Image(systemName: icon(n))
                .foregroundStyle(n.kind == UInt8(DF_KIND_DENIED) ? AnyShapeStyle(.orange) : AnyShapeStyle(.secondary))
            Text(row.name)
                .lineLimit(1)
                .fontWeight(n.hit ? .semibold : .regular)
                .foregroundStyle(n.hit ? AnyShapeStyle(.orange) : dimmed ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.primary))
            if let hint = row.hint {
                Text(hint)
                    .font(.caption2.weight(.medium))
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .glassEffect(.regular.tint(.blue.opacity(0.5)), in: .capsule)
            }
        }
        .help(model.path(row.id))
    }

    private func icon(_ n: DfNode) -> String {
        switch Int(n.kind) {
        case DF_KIND_DENIED: "lock.fill"
        case DF_KIND_SYMLINK: "arrow.turn.up.right"
        case DF_KIND_FILE: "doc"
        default: "folder"
        }
    }
}

enum InsightsTab: String, CaseIterable, Identifiable {
    case hints = "Cleanup", largest = "Largest", types = "Types"
    var id: String { rawValue }
}

struct InsightsView: View {
    let model: Model
    @State private var tab = InsightsTab.hints

    var body: some View {
        VStack(spacing: 0) {
            Picker("Insights", selection: $tab) {
                ForEach(InsightsTab.allCases) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(10)
            switch tab {
            case .hints: hints
            case .largest: list(model.largest, header: "Top \(model.largest.count) files")
            case .types: types
            }
        }
    }

    private var hints: some View {
        let total = model.candidates.reduce(0) { $0 + (model.node($1)?.size ?? 0) }
        return VStack(spacing: 0) {
            HStack {
                Text("\(model.candidates.count) found, \(formatBytes(total))").foregroundStyle(.secondary)
                Spacer()
                Button("Select All") { model.selection = Set(model.candidates) }
                    .buttonStyle(.glass)
                    .help("Then press ⌫ to move them to the Trash")
                    .disabled(model.candidates.isEmpty)
            }
            .padding(.horizontal, 12)
            .padding(.bottom, 6)
            list(model.candidates, header: nil)
        }
    }

    private func list(_ ids: [UInt32], header: String?) -> some View {
        let root = model.path(0)
        return List {
            if let header { Text(header).foregroundStyle(.secondary) }
            ForEach(ids, id: \.self) { id in
                if let n = model.node(id) {
                    Button { model.jump(id) } label: {
                        HStack {
                            Text(formatBytes(n.size)).monospacedDigit().frame(width: 72, alignment: .trailing)
                            Text(String(model.path(id).dropFirst(root.count).drop { $0 == "/" }))
                                .lineLimit(1).truncationMode(.middle)
                            Spacer()
                            if let h = n.hintString { Text(h).font(.caption).foregroundStyle(.secondary) }
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .listStyle(.inset)
    }

    private var types: some View {
        List(model.types) { t in
            Button {
                if t.ext != "(none)" { model.filter = ".\(t.ext)" }
            } label: {
                HStack {
                    Text(formatBytes(t.bytes)).monospacedDigit().frame(width: 72, alignment: .trailing)
                    Text(".\(t.ext)")
                    Spacer()
                    Text("\(t.files) files").foregroundStyle(.secondary)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Filter the tree by this extension")
        }
        .listStyle(.inset)
    }
}

/// Floating glass bar: disk usage, selection total and status.
struct InfoBar: View {
    let model: Model

    var body: some View {
        let sel = model.selectionSize()
        HStack(spacing: 14) {
            if let d = model.diskUsage() {
                Gauge(value: Double(d.total - d.free), in: 0...Double(max(d.total, 1))) { EmptyView() }
                    .gaugeStyle(.accessoryLinearCapacity)
                    .frame(width: 90)
                Text("\(formatBytes(d.total - d.free)) used of \(formatBytes(d.total)), \(formatBytes(d.free)) free")
            }
            if sel.count > 0 {
                Divider().frame(height: 14)
                Text("\(sel.count) selected, \(formatBytes(sel.bytes))").fontWeight(.semibold)
            }
            if !model.status.isEmpty {
                Divider().frame(height: 14)
                Text(model.status).foregroundStyle(.secondary)
            }
        }
        .font(.callout)
        .monospacedDigit()
        .padding(.horizontal, 18)
        .padding(.vertical, 9)
        .glassEffect(.regular, in: .capsule)
        .padding(.bottom, 16)
    }
}

struct ScanProgress: View {
    let model: Model

    var body: some View {
        HStack(spacing: 10) {
            ProgressView().controlSize(.small)
            Text("Scanning… \(model.scannedFiles) files, \(formatBytes(model.scannedBytes))")
                .monospacedDigit()
            Button("Cancel", systemImage: "xmark") { model.cancel() }
                .labelStyle(.iconOnly)
                .buttonStyle(.glass)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 10)
        .glassEffect(.regular, in: .capsule)
        .padding(.bottom, 24)
    }
}

struct Toolbar: ToolbarContent {
    let model: Model

    var body: some ToolbarContent {
        @Bindable var model = model
        ToolbarItem(placement: .navigation) {
            Menu {
                ForEach(Volume.mounted()) { v in
                    Button("\(v.name) — \(formatBytes(v.free)) free") { model.rootPath = v.path }
                }
            } label: {
                Label("Volume", systemImage: "internaldrive")
            }
        }
        ToolbarItem(placement: .principal) {
            TextField("Path", text: $model.rootPath)
                .textFieldStyle(.roundedBorder)
                .frame(width: 300)
                .onSubmit { model.scan() }
        }
        ToolbarItemGroup {
            if model.scanning {
                Button("Cancel", systemImage: "stop.fill") { model.cancel() }
            } else {
                Button("Scan", systemImage: "arrow.clockwise") { model.scan() }
            }
            Picker("Sort", selection: $model.sort) {
                ForEach(SortMode.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.menu)
            Toggle("Insights", systemImage: "sidebar.right", isOn: $model.showInsights)
            Menu {
                Picker("Dim recent", selection: $model.dimDays) {
                    Text("Off").tag(0)
                    ForEach([7, 30, 90, 365], id: \.self) { Text("Changed in last \($0) days").tag($0) }
                }
                .pickerStyle(.inline)
            } label: {
                Label("Dim recent", systemImage: "clock.arrow.circlepath")
            }
        }
    }
}
