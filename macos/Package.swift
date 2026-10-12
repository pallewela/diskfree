// swift-tools-version: 6.0
// Native macOS front end. Build the Rust core first: `cargo build --release --lib` (repo root).
import Foundation
import PackageDescription

let repo = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().path

let package = Package(
    name: "Diskfree",
    platforms: [.macOS("26.0")],
    targets: [
        // C module over include/diskfree.h, linking target/release/libdiskfree.a.
        .systemLibrary(name: "DiskfreeCore", path: "Sources/DiskfreeCore"),
        .executableTarget(
            name: "Diskfree",
            dependencies: ["DiskfreeCore"],
            linkerSettings: [.unsafeFlags(["-L\(repo)/target/release"])]
        ),
    ]
)
