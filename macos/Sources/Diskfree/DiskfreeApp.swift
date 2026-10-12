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
        // Running as a bare SwiftPM executable: become a regular, frontmost app.
        NSApplication.shared.setActivationPolicy(.regular)
        DispatchQueue.main.async { NSApp.activate() }
    }

    var body: some Scene {
        WindowGroup("diskfree") {
            ContentView()
                .environment(model)
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

struct ContentView: View {
    @Environment(Model.self) private var model

    var body: some View {
        @Bindable var model = model
        ZStack(alignment: .bottom) {
            if model.hasTree {
                TreeTable()
            } else {
                ContentUnavailableView("Pick a volume or a folder", systemImage: "internaldrive",
                                       description: Text("Then press Scan."))
            }
            if model.scanning {
                ScanProgress()
            } else if model.hasTree {
                InfoBar()
            }
        }
        .toolbar { Toolbar() }
        .searchable(text: $model.filter, placement: .toolbar, prompt: "Filter names")
    }
}

struct TreeTable: View {
    @Environment(Model.self) private var model

    var body: some View {
        @Bindable var model = model
        let now = UInt64(Date().timeIntervalSince1970)
        let recent = model.dimDays == 0 ? UInt64.max : now - UInt64(model.dimDays) * 86_400
        Table(model.rows, selection: $model.selection) {
            TableColumn("Name") { row in
                NameCell(row: row, dimmed: row.node.mtime > recent)
            }
            .width(min: 260, ideal: 480)
            TableColumn("Size") { row in
                Text(formatBytes(row.node.size)).monospacedDigit()
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(min: 70, ideal: 90)
            TableColumn("%") { row in
                let p = row.node.parent_size == 0 ? 0 : Double(row.node.size) / Double(row.node.parent_size)
                ProgressView(value: min(p, 1)).progressViewStyle(.linear)
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
        .contextMenu(forSelectionType: UInt32.self) { _ in
        } primaryAction: { ids in
            // Double-click / Return toggles folders.
            for id in ids { model.toggle(id) }
        }
        .onKeyPress(.rightArrow) { model.setExpanded(model.selection, true); return .handled }
        .onKeyPress(.leftArrow) { model.setExpanded(model.selection, false); return .handled }
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

struct NameCell: View {
    @Environment(Model.self) private var model
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

/// Floating glass bar: disk usage, selection total and status.
struct InfoBar: View {
    @Environment(Model.self) private var model

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
    @Environment(Model.self) private var model

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
    @Environment(Model.self) private var model

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
