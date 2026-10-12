use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{mpsc, Mutex};
use std::time::UNIX_EPOCH;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Dir,
    File,
    Symlink,
    Denied,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Sort {
    Size,
    Name,
    Files,
}

pub struct Node {
    pub name: Box<str>,
    pub size: u64,
    pub files: u64,
    pub parent: u32,
    pub children: Vec<u32>,
    pub kind: Kind,
    pub mtime: u64, // secs since epoch; for dirs, the newest change anywhere inside
}

// ponytail: ~80 B/node + name; ~400 MB for 5M entries. Intern names / pack if that hurts.
pub struct Tree {
    pub nodes: Vec<Node>,
    pub root: PathBuf,
}

/// Shared with the UI thread while a scan runs.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

/// Mount points that are virtual or duplicate data already reachable elsewhere.
const SKIP: &[&str] = if cfg!(target_os = "macos") {
    // /System/Volumes/Data is firmlinked into /, /Volumes holds other disks.
    &["/System/Volumes/Data", "/Volumes", "/dev"]
} else if cfg!(target_os = "linux") {
    &["/proc", "/sys", "/dev"]
} else {
    &[]
};

fn mtime(md: &Metadata) -> u64 {
    md.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

#[cfg(unix)]
fn alloc_size(md: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.blocks() * 512
}
#[cfg(not(unix))]
fn alloc_size(md: &Metadata) -> u64 {
    md.len()
}

/// True if this inode was already counted (hardlink). Unix only.
type Seen = Mutex<HashSet<(u64, u64)>>;

#[cfg(unix)]
fn seen_before(md: &Metadata, seen: &Seen) -> bool {
    use std::os::unix::fs::MetadataExt;
    md.nlink() > 1 && !seen.lock().unwrap().insert((md.dev(), md.ino()))
}
// ponytail: Windows counts hardlinks twice; use file index via GetFileInformationByHandle if it matters.
#[cfg(not(unix))]
fn seen_before(_: &Metadata, _: &Seen) -> bool {
    false
}

fn dir_node(name: Box<str>, mtime: u64) -> Node {
    Node { name, size: 0, files: 0, parent: u32::MAX, children: Vec::new(), kind: Kind::Dir, mtime }
}

fn skipped(path: &Path) -> bool {
    SKIP.iter().any(|s| path == Path::new(s))
}

/// Sum sizes/counts/mtime of children into `id` and sort them largest first.
fn rollup(nodes: &mut [Node], id: u32) {
    let mut kids = std::mem::take(&mut nodes[id as usize].children);
    kids.sort_unstable_by_key(|&c| std::cmp::Reverse(nodes[c as usize].size));
    let size = kids.iter().map(|&c| nodes[c as usize].size).sum();
    let files = kids.iter().map(|&c| nodes[c as usize].files).sum();
    let newest = kids.iter().map(|&c| nodes[c as usize].mtime).max().unwrap_or(0);
    let n = &mut nodes[id as usize];
    n.children = kids;
    n.size = size;
    n.files = files;
    n.mtime = n.mtime.max(newest);
}

/// One directory's contents, read by a worker thread.
enum Entry {
    Dir(Box<str>, u64, PathBuf),    // name, mtime, path to read next
    Leaf(Box<str>, Kind, u64, u64), // name, kind, size, mtime
}

/// Worker side: the slow part (read_dir + stat). `None` = permission denied / unreadable.
fn read_entries(dir: &Path, p: &Progress, seen: &Seen) -> Option<Vec<Entry>> {
    if p.cancel.load(Relaxed) {
        return Some(Vec::new());
    }
    *p.current.lock().unwrap() = dir.to_string_lossy().into_owned();
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).ok()?.flatten() {
        // DirEntry::metadata does not follow symlinks.
        let Ok(md) = entry.metadata() else { continue };
        let name: Box<str> = entry.file_name().to_string_lossy().into();
        if md.is_dir() {
            let path = entry.path();
            if !skipped(&path) {
                out.push(Entry::Dir(name, mtime(&md), path));
            }
        } else {
            let kind = if md.is_symlink() { Kind::Symlink } else { Kind::File };
            let size = if seen_before(&md, seen) { 0 } else { alloc_size(&md) };
            p.files.fetch_add(1, Relaxed);
            p.bytes.fetch_add(size, Relaxed);
            out.push(Entry::Leaf(name, kind, size, mtime(&md)));
        }
    }
    Some(out)
}

impl Tree {
    /// Worker threads read directories; this thread appends every node to the arena exactly once
    /// (no subtree copying), then sizes are rolled up in one reverse pass.
    pub fn scan(root: &Path, p: &Progress) -> Tree {
        let mut t = Tree { nodes: vec![dir_node(root.to_string_lossy().into(), 0)], root: root.to_path_buf() };
        let seen: Seen = Mutex::new(HashSet::new());
        let (job_tx, job_rx) = mpsc::channel::<(u32, PathBuf)>();
        let job_rx = Mutex::new(job_rx);
        let (res_tx, res_rx) = mpsc::channel();
        // Measured on an 18-core Mac scanning /: 4 thr 29s, 8 thr 20s, 12+ thr no faster (kernel
        // FS lock contention). Sequential was 55s.
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
        std::thread::scope(|s| {
            for _ in 0..workers {
                let (res_tx, job_rx, seen) = (res_tx.clone(), &job_rx, &seen);
                s.spawn(move || {
                    loop {
                        // Bind first: in `while let` the guard would live through the body and serialize workers.
                        let job = job_rx.lock().unwrap().recv();
                        let Ok((id, path)) = job else { break };
                        let _ = res_tx.send((id, read_entries(&path, p, seen)));
                    }
                });
            }
            job_tx.send((0, root.to_path_buf())).unwrap();
            let mut pending = 1;
            while pending > 0 {
                let (id, entries) = res_rx.recv().unwrap();
                pending -= 1;
                let Some(entries) = entries else {
                    t.nodes[id as usize].kind = Kind::Denied;
                    continue;
                };
                for e in entries {
                    match e {
                        Entry::Dir(name, mtime, path) => {
                            let c = t.push(name, id, Kind::Dir, 0, mtime);
                            job_tx.send((c, path)).unwrap();
                            pending += 1;
                        }
                        Entry::Leaf(name, kind, size, mtime) => {
                            t.push(name, id, kind, size, mtime);
                        }
                    }
                }
            }
            drop(job_tx); // workers' recv() fails -> they exit
        });
        // Children always have higher ids than parents, so reverse order is a valid post-order.
        for id in (0..t.nodes.len() as u32).rev() {
            if !t.nodes[id as usize].children.is_empty() {
                rollup(&mut t.nodes, id);
            }
        }
        t
    }

    fn push(&mut self, name: Box<str>, parent: u32, kind: Kind, size: u64, mtime: u64) -> u32 {
        let id = self.nodes.len() as u32;
        let files = (kind != Kind::Dir && kind != Kind::Denied) as u64;
        self.nodes.push(Node { name, size, files, parent, children: Vec::new(), kind, mtime });
        self.nodes[parent as usize].children.push(id);
        id
    }

    pub fn sort_children(&mut self, id: u32, by: Sort) {
        let mut kids = std::mem::take(&mut self.nodes[id as usize].children);
        let n = &self.nodes;
        match by {
            Sort::Size => kids.sort_by_key(|&c| std::cmp::Reverse(n[c as usize].size)),
            Sort::Files => kids.sort_by_key(|&c| std::cmp::Reverse(n[c as usize].files)),
            Sort::Name => kids.sort_by_cached_key(|&c| n[c as usize].name.to_lowercase()),
        }
        self.nodes[id as usize].children = kids;
    }

    pub fn sort_all(&mut self, by: Sort) {
        for id in 0..self.nodes.len() as u32 {
            self.sort_children(id, by);
        }
    }

    /// Apply a size/count change to every ancestor starting at `from`, re-sorting along the way.
    fn adjust_up(&mut self, mut from: u32, dsize: i64, dfiles: i64, by: Sort) {
        while from != u32::MAX {
            let n = &mut self.nodes[from as usize];
            n.size = n.size.saturating_add_signed(dsize);
            n.files = n.files.saturating_add_signed(dfiles);
            self.sort_children(from, by);
            from = self.nodes[from as usize].parent;
        }
    }

    /// Drop a node (after it was trashed). Its arena slot is orphaned, not freed.
    pub fn remove(&mut self, id: u32, by: Sort) {
        let n = &mut self.nodes[id as usize];
        let (parent, size, files) = (n.parent, n.size, n.files);
        n.parent = u32::MAX;
        self.nodes[parent as usize].children.retain(|&c| c != id);
        self.adjust_up(parent, -(size as i64), -(files as i64), by);
    }

    /// Replace the subtree under `id` with a fresh scan of the same path.
    // ponytail: old subtree stays orphaned in the arena; full rescan reclaims it.
    pub fn graft(&mut self, id: u32, sub: Tree, by: Sort) {
        for c in std::mem::take(&mut self.nodes[id as usize].children) {
            self.nodes[c as usize].parent = u32::MAX;
        }
        let off = self.nodes.len() as u32 - 1;
        let map = |i: u32| if i == 0 { id } else { i + off };
        let mut it = sub.nodes.into_iter();
        let top = it.next().unwrap();
        for mut n in it {
            n.parent = map(n.parent);
            n.children.iter_mut().for_each(|c| *c = map(*c));
            self.nodes.push(n);
        }
        if by != Sort::Size {
            for i in off + 1..self.nodes.len() as u32 {
                self.sort_children(i, by);
            }
        }
        let old = &mut self.nodes[id as usize];
        let (dsize, dfiles) = (top.size as i64 - old.size as i64, top.files as i64 - old.files as i64);
        old.children = top.children.into_iter().map(map).collect();
        old.kind = top.kind;
        old.size = top.size;
        old.files = top.files;
        old.mtime = top.mtime;
        let parent = old.parent;
        self.sort_children(id, by);
        self.adjust_up(parent, dsize, dfiles, by);
    }

    /// Still attached to the tree (not trashed or replaced by a rescan).
    pub fn is_live(&self, mut id: u32) -> bool {
        while id != 0 {
            id = self.nodes[id as usize].parent;
            if id == u32::MAX {
                return false;
            }
        }
        true
    }

    pub fn path(&self, mut id: u32) -> PathBuf {
        let mut parts = Vec::new();
        while id != 0 {
            let n = &self.nodes[id as usize];
            parts.push(&*n.name);
            id = n.parent;
        }
        let mut p = self.root.clone();
        p.extend(parts.iter().rev());
        p
    }
}

pub fn fmt_size(b: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_order_links() {
        let d = std::env::temp_dir().join(format!("diskfree-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("big/inner/d1/d2")).unwrap();
        fs::write(d.join("big/inner/d1/d2/c.bin"), vec![1u8; 30_000]).unwrap();
        fs::create_dir_all(d.join("small")).unwrap();
        fs::create_dir_all(d.join("outside")).unwrap();
        fs::write(d.join("big/inner/a.bin"), vec![1u8; 200_000]).unwrap();
        fs::write(d.join("small/b.bin"), vec![1u8; 10_000]).unwrap();
        fs::write(d.join("outside/huge.bin"), vec![1u8; 500_000]).unwrap();

        let scan_root = d.join("root");
        fs::create_dir_all(&scan_root).unwrap();
        fs::rename(d.join("big"), scan_root.join("big")).unwrap();
        fs::rename(d.join("small"), scan_root.join("small")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(d.join("outside"), scan_root.join("link")).unwrap();
            fs::hard_link(scan_root.join("big/inner/a.bin"), scan_root.join("big/a-hard.bin")).unwrap();
        }

        let t = Tree::scan(&scan_root, &Progress::default());
        let a = alloc_size(&fs::metadata(scan_root.join("big/inner/a.bin")).unwrap());
        let b = alloc_size(&fs::metadata(scan_root.join("small/b.bin")).unwrap());
        let c = alloc_size(&fs::metadata(scan_root.join("big/inner/d1/d2/c.bin")).unwrap());
        let root = &t.nodes[0];
        let names: Vec<&str> = root.children.iter().map(|&c| &*t.nodes[c as usize].name).collect();
        assert_eq!(names[0], "big", "largest first: {names:?}");
        // Symlink not followed (no 500 KB), hardlink counted once.
        let link = if cfg!(unix) { alloc_size(&fs::symlink_metadata(scan_root.join("link")).unwrap()) } else { 0 };
        assert_eq!(root.size, a + b + c + link);
        let big = &t.nodes[root.children[0] as usize];
        let inner = *big.children.iter().find(|&&c| &*t.nodes[c as usize].name == "inner").unwrap();
        assert_eq!(t.path(inner), scan_root.join("big/inner"));

        // Remove "small", then rescan "big" after deleting one of its files.
        let mut t = t;
        let small = t.nodes[0].children[1];
        t.remove(small, Sort::Size);
        assert_eq!(t.nodes[0].size, a + c + link);
        assert!(!t.is_live(small));
        fs::remove_file(scan_root.join("big/inner/a.bin")).unwrap();
        let big = t.nodes[0].children[0];
        let sub = Tree::scan(&t.path(big), &Progress::default());
        let big_new = sub.nodes[0].size;
        t.graft(big, sub, Sort::Size);
        assert_eq!(t.nodes[big as usize].size, big_new);
        assert_eq!(t.nodes[0].size, big_new + link);
        fs::remove_dir_all(&d).unwrap();
    }
}
