//! C ABI for the native macOS app. Mirrors `include/diskfree.h`.
//!
//! Handles are opaque heap pointers owned by the caller until passed to the matching
//! `*_free` / `*_finish` function. Name/hint pointers returned in `DfNode` and `DfType` stay
//! valid until the next call that mutates the explorer (scan finish, remove, graft, sort).
//! Nothing here is thread-safe: use one explorer from one thread.

use crate::explorer::Explorer;
use crate::insights;
use crate::scan::{Kind, ScanJob, Sort};
use std::collections::HashSet;
use std::ffi::{c_char, CStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

#[repr(C)]
pub struct DfNode {
    pub size: u64,
    pub files: u64,
    pub mtime: u64,
    pub parent_size: u64,
    pub parent: u32,
    pub child_count: u32,
    pub kind: u8, // 0 dir, 1 file, 2 symlink, 3 denied
    pub expanded: bool,
    pub hit: bool,
    pub name: *const u8,
    pub name_len: usize,
    pub hint: *const u8, // null if none
    pub hint_len: usize,
}

#[repr(C)]
pub struct DfType {
    pub ext: *const u8,
    pub ext_len: usize,
    pub bytes: u64,
    pub files: u64,
}

fn sort_from(v: u8) -> Sort {
    match v {
        1 => Sort::Name,
        2 => Sort::Files,
        _ => Sort::Size,
    }
}

unsafe fn str_arg<'a>(s: *const c_char) -> std::borrow::Cow<'a, str> {
    if s.is_null() { "".into() } else { unsafe { CStr::from_ptr(s) }.to_string_lossy() }
}

/// Copy `out` into the caller's slice (up to `cap`) and return the full count.
unsafe fn write_ids(out: *mut u32, cap: usize, ids: impl ExactSizeIterator<Item = u32>) -> usize {
    let n = ids.len();
    for (i, id) in ids.enumerate().take(if out.is_null() { 0 } else { cap }) {
        unsafe { *out.add(i) = id };
    }
    n
}

// ---- scanning ----

/// Scan `path` (UTF-8) in the background.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_start(path: *const c_char) -> *mut ScanJob {
    let path = PathBuf::from(unsafe { str_arg(path) }.into_owned());
    Box::into_raw(Box::new(ScanJob::start(vec![path])))
}

/// Rescan the subtree `id` of `ex` in the background; finish with `df_scan_finish_graft`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_rescan_start(ex: *const Explorer, id: u32) -> *mut ScanJob {
    let ex = unsafe { &*ex };
    Box::into_raw(Box::new(ScanJob::start(vec![ex.tree.path(id)])))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_progress(job: *const ScanJob, files: *mut u64, bytes: *mut u64) {
    let p = &unsafe { &*job }.progress;
    unsafe {
        *files = p.files.load(Relaxed);
        *bytes = p.bytes.load(Relaxed);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_cancel(job: *const ScanJob) {
    unsafe { &*job }.cancel();
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_done(job: *const ScanJob) -> bool {
    unsafe { &*job }.is_done()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_cancelled(job: *const ScanJob) -> bool {
    unsafe { &*job }.cancelled()
}

/// Consume the job (blocks until done) and return a new explorer. Partial if cancelled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_finish(job: *mut ScanJob, sort: u8) -> *mut Explorer {
    let job = unsafe { Box::from_raw(job) };
    let tree = job.finish().pop().expect("one root");
    Box::into_raw(Box::new(Explorer::new(tree, sort_from(sort))))
}

/// Consume a `df_rescan_start` job and graft it into `ex`. A cancelled rescan is discarded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_scan_finish_graft(job: *mut ScanJob, ex: *mut Explorer, id: u32) {
    let job = unsafe { Box::from_raw(job) };
    let cancelled = job.cancelled();
    let tree = job.finish().pop().expect("one root");
    if !cancelled {
        unsafe { &mut *ex }.graft(vec![(id, tree)]);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_explorer_free(ex: *mut Explorer) {
    if !ex.is_null() {
        drop(unsafe { Box::from_raw(ex) });
    }
}

// ---- rows & nodes ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_row_count(ex: *const Explorer) -> usize {
    unsafe { &*ex }.rows.len()
}

/// Row `i` -> node id and depth. False if out of range.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_row(ex: *const Explorer, i: usize, id: *mut u32, depth: *mut u16) -> bool {
    let Some(&(n, d)) = unsafe { &*ex }.rows.get(i) else { return false };
    unsafe {
        *id = n;
        *depth = d;
    }
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_node(ex: *const Explorer, id: u32, out: *mut DfNode) -> bool {
    let ex = unsafe { &*ex };
    let t = &ex.tree;
    let Some(n) = t.nodes.get(id as usize) else { return false };
    let hint = insights::hint(t, id).unwrap_or("");
    unsafe {
        *out = DfNode {
            size: n.size,
            files: n.files,
            mtime: n.mtime,
            parent_size: t.nodes.get(n.parent as usize).map_or(n.size, |p| p.size),
            parent: n.parent,
            child_count: n.children.len() as u32,
            kind: match n.kind {
                Kind::Dir => 0,
                Kind::File => 1,
                Kind::Symlink => 2,
                Kind::Denied => 3,
            },
            expanded: ex.expanded[id as usize],
            hit: ex.hit[id as usize],
            name: n.name.as_ptr(),
            name_len: n.name.len(),
            hint: if hint.is_empty() { std::ptr::null() } else { hint.as_ptr() },
            hint_len: hint.len(),
        }
    };
    true
}

/// Write the full path of `id` (UTF-8, not NUL-terminated) into `buf`; returns the needed length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_path(ex: *const Explorer, id: u32, buf: *mut u8, cap: usize) -> usize {
    let path = unsafe { &*ex }.tree.path(id);
    let s = path.to_string_lossy();
    if !buf.is_null() {
        let n = s.len().min(cap);
        unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), buf, n) };
    }
    s.len()
}

// ---- browsing ----

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_toggle(ex: *mut Explorer, id: u32, open: bool) {
    unsafe { &mut *ex }.toggle(id, open);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_set_sort(ex: *mut Explorer, sort: u8) {
    unsafe { &mut *ex }.set_sort(sort_from(sort));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_set_filter(ex: *mut Explorer, query: *const c_char) {
    let q = unsafe { str_arg(query) };
    unsafe { &mut *ex }.set_filter(&q);
}

/// Expand down to `id` (clears the filter); returns its row index or -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_reveal(ex: *mut Explorer, id: u32) -> i64 {
    unsafe { &mut *ex }.reveal(id).map_or(-1, |i| i as i64)
}

/// Forget nodes the caller already deleted from disk; totals and rows update.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_remove(ex: *mut Explorer, ids: *const u32, n: usize) {
    let ids = unsafe { std::slice::from_raw_parts(ids, n) };
    unsafe { &mut *ex }.remove(ids);
}

/// Reduce a selection to its top-level ids (no id whose ancestor is also given).
/// Writes up to `cap` ids to `out`; returns the full count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_top_level(ex: *const Explorer, ids: *const u32, n: usize, out: *mut u32, cap: usize) -> usize {
    let sel: HashSet<u32> = unsafe { std::slice::from_raw_parts(ids, n) }.iter().copied().collect();
    let top = unsafe { &*ex }.tree.top_level(&sel);
    unsafe { write_ids(out, cap, top.into_iter()) }
}

/// Node id for a path (UTF-8), or `UINT32_MAX` if it is not in the tree.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_find(ex: *const Explorer, path: *const c_char) -> u32 {
    let path = unsafe { str_arg(path) };
    unsafe { &*ex }.tree.find(Path::new(&*path)).unwrap_or(u32::MAX)
}

/// Re-read folders in place after they changed on disk; ids of surviving nodes stay the same.
/// Writes up to `cap` new subfolder ids (they still need `df_rescan_start`); returns the full count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_refresh_dirs(ex: *mut Explorer, ids: *const u32, n: usize, out: *mut u32, cap: usize) -> usize {
    let ids = unsafe { std::slice::from_raw_parts(ids, n) };
    let fresh = unsafe { &mut *ex }.refresh_dirs(ids);
    unsafe { write_ids(out, cap, fresh.into_iter()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_is_live(ex: *const Explorer, id: u32) -> bool {
    let t = &unsafe { &*ex }.tree;
    (id as usize) < t.nodes.len() && t.is_live(id)
}

// ---- insights ----

/// Largest files, biggest first. Writes up to `cap` ids; returns the full count.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_largest(ex: *const Explorer, out: *mut u32, cap: usize) -> usize {
    let s = &unsafe { &*ex }.stats;
    unsafe { write_ids(out, cap, s.largest.iter().copied()) }
}

/// Cleanup candidates, biggest first (hint text via `df_node`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_candidates(ex: *const Explorer, out: *mut u32, cap: usize) -> usize {
    let s = &unsafe { &*ex }.stats;
    unsafe { write_ids(out, cap, s.candidates.iter().map(|c| c.0)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_type_count(ex: *const Explorer) -> usize {
    unsafe { &*ex }.stats.types.len()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn df_type(ex: *const Explorer, i: usize, out: *mut DfType) -> bool {
    let Some((ext, bytes, files)) = unsafe { &*ex }.stats.types.get(i) else { return false };
    unsafe { *out = DfType { ext: ext.as_ptr(), ext_len: ext.len(), bytes: *bytes, files: *files } };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    unsafe fn name(n: &DfNode) -> &str {
        unsafe { std::str::from_utf8(std::slice::from_raw_parts(n.name, n.name_len)).unwrap() }
    }

    #[test]
    fn scan_browse_remove_rescan() {
        let d = std::env::temp_dir().join(format!("diskfree-ffi-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("node_modules")).unwrap();
        fs::write(d.join("node_modules/big.js"), vec![0u8; 100_000]).unwrap();
        fs::write(d.join("small.txt"), vec![0u8; 5_000]).unwrap();
        let root = std::ffi::CString::new(d.to_str().unwrap()).unwrap();
        unsafe {
            let job = df_scan_start(root.as_ptr());
            let ex = df_scan_finish(job, 0);
            assert_eq!(df_row_count(ex), 1);
            df_toggle(ex, 0, true);
            assert_eq!(df_row_count(ex), 3);

            let (mut id, mut depth) = (0, 0);
            assert!(df_row(ex, 1, &mut id, &mut depth));
            let mut n = std::mem::zeroed::<DfNode>();
            assert!(df_node(ex, id, &mut n));
            assert_eq!((name(&n), depth, n.kind), ("node_modules", 1, 0));
            assert_eq!(std::slice::from_raw_parts(n.hint, n.hint_len), b"npm deps");
            let nm = id;

            let mut buf = [0u8; 512];
            let len = df_path(ex, nm, buf.as_mut_ptr(), buf.len());
            assert_eq!(&buf[..len], d.join("node_modules").to_str().unwrap().as_bytes());

            let mut out = [0u32; 4];
            assert_eq!(df_candidates(ex, out.as_mut_ptr(), 4), 1);
            assert_eq!(out[0], nm);
            assert_eq!(df_top_level(ex, [nm, 0].as_ptr(), 2, out.as_mut_ptr(), 4), 1);
            assert_eq!(out[0], 0);

            // Delete on disk, then tell the explorer.
            fs::remove_dir_all(d.join("node_modules")).unwrap();
            df_remove(ex, &nm, 1);
            assert!(!df_is_live(ex, nm));
            assert_eq!(df_row_count(ex), 2);

            // Rescan root after adding a file.
            fs::write(d.join("new.bin"), vec![0u8; 50_000]).unwrap();
            let job = df_rescan_start(ex, 0);
            df_scan_finish_graft(job, ex, 0);
            df_node(ex, 0, &mut n);
            assert_eq!(n.files, 2);
            assert_eq!(df_type_count(ex), 2);

            df_explorer_free(ex);
        }
        fs::remove_dir_all(&d).unwrap();
    }
}
