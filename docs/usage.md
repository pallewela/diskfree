# Using diskfree

## Scanning

1. Pick a disk from the **Volume** menu. Each disk shows its free space. You can also type a path into the field next to it.
2. Press **Scan** or hit Enter. While the scan runs, a progress line shows the number of files and bytes found and the folder being read.
3. **Cancel** stops the scan. For a full scan, the partial results are kept.

You can also start scanning on launch with `diskfree <path>`.

After a scan, the header shows the used, total and free space of the disk that holds the scan root, with a usage bar.

## Reading the tree

The tree starts with one collapsed row: the root. Each row has these columns:

| Column | Meaning |
|---|---|
| Size | Space allocated on disk by everything inside |
| Bar | Share of the parent folder's size |
| Files | Number of files inside, at any depth |
| Age | Time since the newest change anywhere inside (`today`, `12d`, `5mo`, `3y`) |
| Name | `▶`/`▼` folder (collapsed/expanded), `🔒` unreadable folder, `→` symlink (not followed) |

A blue badge after the name marks a known cleanup candidate (see [Insights](#insights)).

Click a folder to expand or collapse it. Hover over a row to see its full path.

**Sort** (Size / Name / Files) re-orders the children of every folder.

## Selecting

| Action | Effect |
|---|---|
| Click | Select only this row |
| ⌘-click (Ctrl-click on Linux/Windows) | Add or remove this row |
| Shift-click | Select every row between the last clicked row and this one |
| ↑ / ↓ | Move and select. Hold Shift to extend the selection |
| ⌘/Ctrl-A | Select all visible rows |
| Esc | Clear the selection |

The header shows **N selected, size**. If you select a folder and something inside it, the inner item is ignored, so nothing is counted twice.

## Actions

Right-click a row to open the menu. If the row is not already selected, it becomes the selection. Every action applies to the whole selection.

- **Reveal in file manager:** opens Finder, Explorer or your file manager at each item.
- **Copy path:** copies the paths, one per line.
- **Rescan:** re-reads the selected folders in the background and updates their totals. A cancelled rescan keeps the old numbers.
- **Move to Trash…:** asks first, showing up to 10 paths, the total size and the file count. Items go to the system Trash or Recycle Bin, so you can restore them. Totals update right away. If some items fail, the rest are still moved, and the status line shows how many failed and the first error.

**Delete** or **Backspace** opens the same Move to Trash confirmation for the selection. In the dialog, Enter confirms and Esc cancels.

## Filter

Type in **filter names…** to match names, case-insensitive. Matching rows are highlighted, and their parent folders expand automatically. A matching folder shows everything inside it. Clear the box to see the whole tree again.

## Dim recent

**Dim recent** greys out rows changed in the last 7, 30, 90 or 365 days. Old, forgotten items stay bright. Rows are dimmed, not hidden, because a folder containing one new file can still hold large old siblings.

## Insights

Toggle the right-hand panel with **Insights**.

- **Cleanup hints:** well-known content that can usually be regenerated, largest first, with a total.
  - `node_modules`, `.venv`/`venv`
  - `target/`, only when there is a `Cargo.toml` next to it
  - `.cache`, `.npm`, `.pnpm-store`, `.gradle`, `Library/Caches`, `__pycache__`
  - Xcode `DerivedData` and `iOS DeviceSupport`
  - `.Trash`, `Docker.raw`, and `tmp` directly under the scan root
  
  Click an entry to jump to it in the tree. **Select all** followed by Delete moves them all to the Trash. These are hints only, so check before deleting.
- **Largest files:** the 200 largest files in the scan. Click one to jump to it.
- **File types:** total size and file count per extension. Click one to filter the tree by that extension.

## Troubleshooting

- **Many 🔒 rows on macOS.** The app (or the terminal you started it from) needs Full Disk Access: System Settings → Privacy & Security → Full Disk Access.
- **Move to Trash asks to control Finder.** That is how the trash library moves items on macOS. Allow it once.
- **Totals are higher than `df` reports.** APFS clones and shared blocks are counted once per file, the same as `du`.
- **Skipped locations.** On macOS, `/System/Volumes/Data` (already visible through `/`), `/Volumes` (other disks) and `/dev` are skipped. On Linux, `/proc`, `/sys` and `/dev` are skipped. To scan another disk, pick it from the Volume menu.
