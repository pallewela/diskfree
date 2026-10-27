# Architecture

diskfree is a Rust library (the core) plus two front ends: the egui binary and a native SwiftUI app in `macos/`.

| File | Role |
|---|---|
| `src/scan.rs` | Core: tree model (arena), parallel scanner, `ScanJob`, mutations (remove, graft, sort), `top_level` |
| `src/explorer.rs` | Core: UI-agnostic browsing state: expanded set, filter marks, visible rows, insights |
| `src/insights.rs` | Core: cleanup hints, largest files, per-extension totals, age formatting |
| `src/ffi.rs` + `include/diskfree.h` | C ABI over `ScanJob` and `Explorer` for the native macOS app |
| `src/lib.rs` | Library root (`rlib` + `staticlib` → `libdiskfree.a`) |
| `src/main.rs` | egui UI, CLI mode, OS actions (trash, reveal, clipboard, disk info) |
| `macos/Sources/Diskfree/Model.swift` | Swift view model over the C ABI: rows, selection, insights, actions |
| `macos/Sources/Diskfree/DiskfreeApp.swift` | SwiftUI views: toolbar, table, inspector, glass bars; `--cli` / `--selftest` |
| `macos/Sources/DiskfreeCore/` | C module map linking `libdiskfree.a` |
| `scripts/build-mac-app.sh` | Builds and ad-hoc signs `target/Diskfree.app`; `--dmg` also packs a disk image |

```
┌─────────────────────────── src/main.rs (egui / eframe) ───────────────────────────┐
│ top bar · tree view (virtualized rows) · insights panel · trash dialog · keys     │
│        │ background Job (std::thread)               │ OS actions                  │
└────────┼────────────────────────────────────────────┼─────────────────────────────┘
         ▼                                            ▼
┌──────── core ────────────────────┐         ┌──────── OS / crates ─────────────────┐
│ scan.rs:  Tree::scan, remove,    │         │ trash    (Trash / Recycle Bin)       │
│           graft, sort, path      │         │ opener   (reveal in file manager)    │
│ insights: compute, hint, fmt_age │         │ sysinfo  (disk list, free space)     │
└────────────┬─────────────────────┘         └──────────────────────────────────────┘
             ▼
        std::fs (read_dir, symlink metadata)
```

## Data model

The whole scan is a single **arena**, `Tree { nodes: Vec<Node>, root: PathBuf }`. Nodes refer to each other by `u32` index.

```rust
pub struct Node {
    name: Box<str>,      // the root node holds the full root path
    size: u64,           // allocated bytes, rolled up for dirs
    files: u64,          // files (incl. symlinks) at any depth
    parent: u32,         // u32::MAX = root or detached
    children: Vec<u32>,  // kept sorted by the current Sort
    kind: Kind,          // Dir | File | Symlink | Denied
    mtime: u64,          // secs since epoch; dirs: newest change inside
}
```

The arena keeps these invariants:

- Node `0` is the root.
- After a scan, a child's id is always greater than its parent's. Grafted nodes are appended, so the rule still holds for them. Scanning uses it to roll up sizes in one reverse pass.
- A node is **live** if walking up its parents reaches `0`. `remove` and `graft` *detach* nodes by setting their `parent` to `u32::MAX` and leave the slots allocated. The next full scan reclaims them. `Tree::is_live` checks this.
- Memory is about 80 bytes per node plus the name: roughly 400 MB for 5 million entries.

`Tree::path(id)` rebuilds the path by walking up the parents, so full paths are never stored.

## Scanner

```
                ┌───────── job channel (id, PathBuf) ─────────┐
                ▼                                              │
  ┌───────────────────────────┐                                │
  │ workers × min(cores, 8)   │  read_entries(dir):            │
  │  read_dir + metadata      │   - cancel check, progress     │
  │  (no symlink following)   │   - Dir → Entry::Dir           │
  └─────────────┬─────────────┘   - other → Entry::Leaf (size) │
                │ result channel (id, Option<Vec<Entry>>)      │
                ▼                                              │
  ┌───────────────────────────┐                                │
  │ builder (calling thread)  │  push each entry into arena ───┘
  │  owns the arena           │  new dir → send job; pending += 1
  └─────────────┬─────────────┘  pending == 0 → done
                ▼
     reverse-id pass: rollup(id) = Σ child size/files, max mtime, sort
```

- **Workers do only the slow part:** reading directories and their metadata. The builder thread owns the arena, so every node is written once and never copied.
- **Sizes** are allocated bytes (`st_blocks × 512`) on Unix, so sparse files are not inflated. On Windows, the file length is used.
- **Hardlinks** with `nlink > 1` are deduplicated by `(dev, ino)` in a shared `Mutex<HashSet>`. The mutex is only taken for multiply-linked files. Windows does not deduplicate.
- **Skipped paths** (`SKIP`):
  - macOS: `/System/Volumes/Data` (firmlinked into `/`, so it would be counted twice), `/Volumes` and `/dev`
  - Linux: `/proc`, `/sys` and `/dev`
  
  The check compares full paths, so scanning `/Volumes/X` directly still works.
- **Unreadable directories** become `Kind::Denied` instead of failing the scan.
- **Cancellation:** a `Progress.cancel` flag makes `read_entries` return empty, and the queue drains within moments.
- **Progress** is shared with the UI through atomics (`files`, `bytes`) and a `Mutex<String>` holding the current directory.

## Mutations

- `remove(id)` detaches the node, subtracts its size and file count from every ancestor (`adjust_up`), and re-sorts each ancestor's children.
- `graft(id, sub)` replaces the subtree under `id` with a fresh scan: it detaches the old children, appends `sub`'s nodes with remapped ids, and applies the size difference to the ancestors.
- `sort_children` and `sort_all` order children by size (descending), name (case-insensitive) or file count (descending).

## Insights

`insights::compute` walks the live tree once and collects three things:

- **Candidates:** `hint(t, id)` matches by name, with a few context rules: `target` needs a sibling `Cargo.toml`, `Caches` must be inside `Library`, and `tmp` must be directly under the root. Nodes below a candidate are not listed again.
- **Largest files:** the top 200 via `select_nth_unstable_by_key`, then sorted.
- **Extension totals:** a `HashMap<ext, (bytes, files)>`.

It is recomputed in full after every tree change. A `ponytail:` comment marks this as the place to switch to incremental updates if it gets slow.

## Explorer (shared browsing state)

`Explorer` owns the `Tree` plus everything derived from it, so every UI behaves the same:

- `expanded: Vec<bool>`
- filter marks `hit` / `anc`
- `rows: Vec<(id, depth)>`
- `stats` (insights)

Its methods are `toggle`, `set_sort`, `set_filter`, `reveal(id) -> row`, `remove(ids)` and `graft(subs)`. Each one keeps the rows, marks and stats consistent.

- **Rows:** `rebuild_rows` flattens the expanded tree into the visible rows.
- **Filter:** a pre-order walk of the live nodes, folded in reverse, marks `hit` (the name matches) and `anc` (has a matching descendant). Ancestors auto-expand, and a matching folder shows its whole subtree.

## UI (egui)

- **Virtualized rows:** `ScrollArea::show_rows` draws only the visible part of `Explorer::rows`.
- **Selection** is a `HashSet<u32>` of node ids, so it survives expanding and collapsing. `Tree::top_level()` drops ids whose ancestor is also selected, so actions never double-count or double-trash. Selected nodes that are no longer live are pruned after every change.
- **Background jobs:** a scan or rescan is a `ScanJob` (a `std::thread`). The UI checks `ScanJob::is_done` each frame and repaints every 100 ms while a job runs. Rescans are grafted in. A cancelled rescan is discarded so it never grafts partial sizes.

## Native macOS UI

```
SwiftUI app (macOS 26, Liquid Glass)        egui app (Linux, Windows, older macOS)
        │  C ABI (include/diskfree.h)                 │  Rust API
        ▼                                             ▼
   src/ffi.rs  ──────────────►  lib: scan.rs · explorer.rs · insights.rs
```

The crate is a library (`rlib` + `staticlib`) with a small C ABI that uses opaque `DfScan`/`DfExplorer` handles (see `include/diskfree.h`). The SwiftUI app (SwiftPM, macOS 26) links `libdiskfree.a` through the `DiskfreeCore` module. egui remains the cross-platform UI.

- **Model** (`@MainActor @Observable`) owns the explorer and scan pointers. After every change it copies the visible rows, candidates, largest files and types out of Rust into Swift arrays, and bumps `revision` so views re-read them. A scan is polled every 100 ms from a main-actor `Task`.
- **Tree:** a `Table` whose rows are keyed by node id. Native ⌘/Shift multi-select is bound straight to `Set<UInt32>`.
  - Expand/collapse calls `df_toggle`.
  - Context menu and double-click use `contextMenu(forSelectionType:primaryAction:)`.
  - Jumping to a node uses `df_reveal` plus `ScrollViewReader`.
- **Actions** use native APIs:
  - `FileManager.trashItem` (no Finder automation prompt), then `df_remove`
  - `NSWorkspace.activateFileViewerSelecting` and `NSPasteboard`
  - Rescan is a queue of `df_rescan_start` / `df_scan_finish_graft` jobs over the top-level selection.
- **Live updates:** an FSEvents stream on the scan root (2 s latency) collects changed folders. Once no job is running, `df_find` maps each one to a node and `df_refresh_dirs` re-reads it shallowly in place (`Tree::refresh_dir`), so surviving nodes keep their ids and expanded state. New subfolders, and folders whose events were dropped (`MustScanSubDirs`), go through the rescan queue. **View ▸ Live Updates** stops and restarts the stream.
- **Live updates in the egui app:** the `notify` crate (inotify, ReadDirectoryChangesW, FSEvents) watches the root recursively. The watcher is set up on a background thread because inotify adds one watch per folder. Each event's parent folder is queued, and `poll_changes` applies the queue between jobs at most every 2 s, through the same `refresh_dirs` and rescan path. Events flagged `need_rescan` (overflow) rescan the folder fully. If the watch fails, for example at the inotify watch limit, the status bar shows *Live updates off* and the app carries on without them.
- **Liquid Glass:** the toolbar and inspector are glass automatically. Custom glass: the scan progress capsule and the info bar (`.glassEffect(.regular, in: .capsule)`), hint badges (`.regular.tint(.blue)`), and `.glass` buttons.
- **Ownership rules:** name and hint pointers in `DfNode` are only valid until the next mutation. Swift copies names into `Row` immediately.

## Decisions and trade-offs

| Decision | Why |
|---|---|
| Worker pool + single builder thread | Measured on `/`: 55 s single-threaded → 20 s at 417 MB peak. Recursive rayon splitting was 16 s but peaked at 1.2 GB because it copied subtree arenas. Rejected. |
| `min(cores, 8)` workers | 12 or more threads gave no extra speed, only more kernel time spent waiting on file-system locks. |
| Bind the job before matching | `while let Ok(j) = rx.lock().unwrap().recv()` holds the mutex guard through the loop body, which made the workers run one at a time (85 s). |
| Allocated size only | It is what deleting frees. An apparent-size toggle was left out until someone needs it. |
| No scan cache | A 20 s rescan is fast enough. |
| Dim, not hide, for age | Hiding a folder because of one new file would hide its large old siblings. |
| Detach instead of compacting | Remove and graft are O(depth). Detached slots are reclaimed by the next full scan. |

Known limits:

- APFS clones and shared blocks are counted once per file, like `du`.
- Windows counts each hardlink in full.
- Each node uses about 80 bytes plus its name.
