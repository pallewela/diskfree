# diskfree

A small, fast disk-usage explorer for when your disk is getting full.

diskfree scans a volume or folder and shows it as an expandable tree. Every row shows the total size of everything inside it, and the largest items are listed first. You can find what is eating space and clean it up from the same window.

It has a Rust core and two front ends:

- **egui app** ([egui](https://github.com/emilk/egui)): runs on macOS, Linux and Windows.
- **Native macOS app** (SwiftUI): for macOS 26 and later, with Liquid Glass: glass toolbar, inspector, floating info bar and badges.

## Features

- **Size tree.** The tree starts collapsed. Each row shows its size, its share of the parent folder, its file count and how long since it last changed. Children are sorted by size, name or file count.
- **Fast scanning.** A pool of worker threads reads the disk. A full scan of `/` (≈790 GiB) takes about 20 s, compared with 55 s for a single-threaded scan. You can cancel at any time and keep the partial results.
- **Real disk usage.** Sizes are the space actually allocated on disk. Symlinks are not followed and hardlinked files are counted once (Unix).
- **Multi-select and batch actions.**
  - Reveal in Finder, Explorer or your file manager.
  - Copy paths.
  - Rescan a folder.
  - **Move to Trash**, with a confirmation. Totals update right away, without a rescan.
- **Live updates.** After a scan, totals follow changes on disk (FSEvents in the macOS app, `notify` in the egui app). Turn it off under **View ▸ Live updates**.
- **Insights panel.**
  - *Cleanup hints:* `node_modules`, Cargo `target/`, caches, Xcode `DerivedData`, `Docker.raw`, `.Trash` and more. Hints are badges only; nothing is deleted automatically.
  - *Largest files:* the top 200 files in the scan.
  - *File types:* total size per extension.
- **Name filter.** Matches are highlighted and the folders above them open automatically.
- **Dim recent.** Greys out items changed in the last 7, 30, 90 or 365 days, so old items stand out.
- **Volume picker.** Shows the free and used space of the scanned disk.

## Build and run

You need a recent stable Rust toolchain.

```sh
cargo run --release              # starts with / (C:\ on Windows); press Scan
cargo run --release -- ~/        # scans the given path on launch
cargo run --release -- --cli ~/  # no GUI: print the 20 largest children
```

### Native macOS app (macOS 26+)

You need the Command Line Tools with the macOS 26 SDK. Xcode is not required.

```sh
./scripts/build-mac-app.sh       # builds target/Diskfree.app
open target/Diskfree.app
```

## Keyboard (egui app)

| Key | Action |
|---|---|
| ↑ / ↓ | Move the cursor (Shift extends the selection) |
| → / ← | Expand / collapse (← on a collapsed row jumps to its parent) |
| ⌘/Ctrl-click, Shift-click | Toggle a row / select a range |
| ⌘/Ctrl-A, Esc | Select all visible rows / clear the selection |
| Delete / Backspace | Move the selection to the Trash (asks first) |

## Permissions and limits

- **macOS:** to scan `/` or other users' folders, grant the app (or your terminal) **Full Disk Access** in System Settings → Privacy & Security. Folders you cannot read show a 🔒. The first Move to Trash may ask for permission to control Finder.
- **APFS clones** are counted once per file, as `du` does, so totals can be higher than what `df` reports.
- On Windows, hardlinked files are counted once per link.

## Documentation

- [Usage guide](docs/usage.md)
- [Architecture](docs/architecture.md)
- [Development](docs/development.md)

## License

[MIT](LICENSE)
