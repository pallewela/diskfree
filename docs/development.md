# Development

## Layout

```
src/lib.rs        library root (rlib + staticlib)
src/scan.rs       tree arena, parallel scanner, ScanJob, remove/graft/sort   (+ tests)
src/explorer.rs   shared browsing state: rows, expand, filter, insights      (+ tests)
src/insights.rs   hints, largest files, extension totals, fmt_age            (+ tests)
src/ffi.rs        C ABI for the macOS app                                     (+ tests)
include/diskfree.h  C header matching src/ffi.rs
src/main.rs       egui app, CLI mode (--cli), OS actions
docs/             usage, architecture, this file
private/          local planning notes (gitignored)
```

See [architecture.md](architecture.md) for how the pieces fit together.

## Build, run, test

```sh
cargo build --release
cargo run --release -- ~/dev          # GUI, scanning ~/dev on launch
cargo run --release -- --cli ~/dev    # print totals; handy for quick checks
cargo test
```

The tests build a temporary directory tree. It includes nested folders, a symlink pointing outside the scan and a hardlink, so the tests check totals, sort order, that symlinks are not followed, hardlink dedup, `remove`, `graft` and `is_live`. They also check the insights pass on a hand-built tree and the `top_level` selection dedup.

To sanity-check totals against the OS:

```sh
./target/release/diskfree --cli ~/dev | head -5
du -sh ~/dev ~/dev/* | sort -rh | head -5
```

## Native macOS app (macOS 26+)

The SwiftUI front end lives in `macos/`, a SwiftPM package. Xcode isn't needed; the Command Line Tools with the macOS 26 SDK are enough. It links the Rust core as `target/release/libdiskfree.a` through the `DiskfreeCore` C module (`include/diskfree.h`).

```sh
./scripts/build-mac-app.sh                 # core + Swift app -> target/Diskfree.app (ad-hoc signed)

# or step by step:
cargo build --release --lib                # rebuild the core whenever src/ changes
cd macos && swift build -c release
.build/release/Diskfree ~/dev              # GUI, scanning on launch
.build/release/Diskfree --cli ~/dev        # headless: view model over the C ABI, prints top rows
.build/release/Diskfree --selftest         # headless end-to-end: trash, rescan, insights, jump on a temp dir
```

`--selftest` moves a 1 MB `junk` folder from a temp directory to your Trash; that's part of the test.

The macOS app shows sizes in decimal units (GB), like Finder. The egui app uses binary units (GiB).

## Benchmarks

Run each case twice with warm caches and report the second run. On macOS, `-l` also prints peak memory:

```sh
/usr/bin/time -l ./target/release/diskfree --cli /
```

Results for `/` (≈790 GiB) on an 18-core Mac running macOS 26:

| Scanner | Time | Peak RSS |
|---|---|---|
| Sequential recursive walk | 55 s | 352 MB |
| rayon recursive split (rejected) | 16 s | 1.2 GB |
| Worker pool, 4 threads | 29 s | 406 MB |
| **Worker pool, 8 threads (current)** | **20 s** | **417 MB** |
| Worker pool, 12 / 18 / 64 threads | 19–22 s | 420–570 MB |

## Cross-platform checks

```sh
rustup target add x86_64-pc-windows-gnu x86_64-unknown-linux-gnu
cargo check --release --target x86_64-pc-windows-gnu
cargo check --release --target x86_64-unknown-linux-gnu
```

Platform-specific code is limited to these spots:

- `alloc_size` and `seen_before`: `cfg(unix)`
- the `SKIP` list: per OS
- the default root: `C:\` on Windows

## Conventions

- Keep it small: standard library first, and a new crate only when it clearly pays for itself. The current crates are `eframe`, `sysinfo`, `trash` and `opener`.
- Comments starting with `ponytail:` mark deliberate shortcuts that have a known limit, along with how to remove that limit. List them with `grep -rn "ponytail:" src`.
- Non-trivial logic gets a small `#[test]`.

## egui 0.36 notes

eframe 0.36 changed APIs that older examples use:

- `App::ui(&mut self, ui, frame)` replaces `update(ctx, frame)`.
- `egui::Panel::top` and `egui::Panel::right` replace `TopBottomPanel` and `SidePanel`.
- `ctx.egui_wants_keyboard_input()` replaces `wants_keyboard_input()`.
