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

/// Headless check of the Rust core through the C ABI: prints the root and its 20 largest children.
@MainActor func cli(_ path: String) {
    guard let ex = df_scan_finish(df_scan_start(path), SortMode.size.rawValue) else { return }
    defer { df_explorer_free(ex) }
    df_toggle(ex, 0, true)
    for i in 0..<min(df_row_count(ex), 21) {
        var id: UInt32 = 0, depth: UInt16 = 0
        var n = DfNode()
        guard df_row(ex, i, &id, &depth), df_node(ex, id, &n) else { continue }
        let size = formatBytes(n.size).padding(toLength: 10, withPad: " ", startingAt: 0)
        print("\(size)  \(n.nameString)\(n.hintString.map { "  [\($0)]" } ?? "")")
    }
}

struct ContentView: View {
    @Environment(Model.self) private var model

    var body: some View {
        @Bindable var model = model
        ZStack(alignment: .bottom) {
            if model.hasTree {
                Summary()
            } else {
                ContentUnavailableView("Pick a volume or a folder", systemImage: "internaldrive",
                                       description: Text("Then press Scan."))
            }
            if model.scanning {
                ScanProgress()
            }
        }
        .toolbar { Toolbar() }
        .searchable(text: $model.filter, placement: .toolbar, prompt: "Filter names")
    }
}

/// Placeholder for the tree (stage 6c): root total plus disk usage.
struct Summary: View {
    @Environment(Model.self) private var model

    var body: some View {
        if let root = model.node(0) {
            VStack(spacing: 8) {
                Text(formatBytes(root.size)).font(.largeTitle.bold())
                Text("\(model.path(0)) — \(root.files) files").foregroundStyle(.secondary)
                if !model.status.isEmpty { Text(model.status).foregroundStyle(.secondary) }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
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
