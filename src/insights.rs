use crate::scan::{Kind, Tree};
use std::collections::HashMap;
use std::path::Path;

pub const TOP_FILES: usize = 200;

#[derive(Default)]
pub struct Stats {
    pub largest: Vec<u32>,
    pub types: Vec<(String, u64, u64)>, // extension, bytes, files
    pub candidates: Vec<(u32, &'static str)>,
}

/// Badge for well-known regenerable/cache content. Advisory only.
pub fn hint(t: &Tree, id: u32) -> Option<&'static str> {
    let n = &t.nodes[id as usize];
    let parent = t.nodes.get(n.parent as usize);
    let dir = n.kind == Kind::Dir;
    let sibling = |name: &str| parent.is_some_and(|p| p.children.iter().any(|&c| &*t.nodes[c as usize].name == name));
    Some(match &*n.name {
        "node_modules" if dir => "npm deps",
        "target" if dir && sibling("Cargo.toml") => "cargo build",
        ".cache" | ".npm" | ".pnpm-store" | ".gradle" if dir => "cache",
        "Caches" if dir && parent.is_some_and(|p| &*p.name == "Library") => "cache",
        "__pycache__" if dir => "python cache",
        ".venv" | "venv" if dir => "python venv",
        "DerivedData" if dir => "Xcode build",
        "iOS DeviceSupport" if dir => "Xcode device support",
        ".Trash" if dir => "trash",
        "Docker.raw" => "Docker disk image",
        "tmp" if dir && n.parent == 0 => "temp",
        _ => return None,
    })
}

/// One pass over the live tree.
// ponytail: recomputed in full after every trash/rescan; incremental if that gets slow on huge trees.
pub fn compute(t: &Tree) -> Stats {
    let mut files = Vec::new();
    let mut types: HashMap<String, (u64, u64)> = HashMap::new();
    let mut candidates = Vec::new();
    let mut stack = vec![(0u32, false)];
    while let Some((id, mut in_cand)) = stack.pop() {
        let n = &t.nodes[id as usize];
        if n.kind == Kind::File {
            files.push(id);
            let ext = Path::new(&*n.name).extension().map_or("(none)".into(), |e| e.to_string_lossy().to_lowercase());
            let e = types.entry(ext).or_default();
            e.0 += n.size;
            e.1 += 1;
        }
        // Don't list nested candidates (node_modules inside node_modules).
        if !in_cand {
            if let Some(h) = hint(t, id) {
                candidates.push((id, h));
                in_cand = true;
            }
        }
        stack.extend(n.children.iter().map(|&c| (c, in_cand)));
    }
    let size = |id: &u32| std::cmp::Reverse(t.nodes[*id as usize].size);
    if files.len() > TOP_FILES {
        files.select_nth_unstable_by_key(TOP_FILES, size);
        files.truncate(TOP_FILES);
    }
    files.sort_unstable_by_key(size);
    candidates.sort_unstable_by_key(|(id, _)| size(id));
    let mut types: Vec<_> = types.into_iter().map(|(e, (b, f))| (e, b, f)).collect();
    types.sort_unstable_by_key(|t| std::cmp::Reverse(t.1));
    Stats { largest: files, types, candidates }
}

pub fn fmt_age(secs: u64) -> String {
    match secs / 86_400 {
        0 => "today".into(),
        d @ 1..=59 => format!("{d}d"),
        d @ 60..=729 => format!("{}mo", d / 30),
        d => format!("{}y", d / 365),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Node;
    use std::path::PathBuf;

    #[test]
    fn stats_and_hints() {
        let node = |name: &str, parent, size, kind, children| Node {
            name: name.into(),
            size,
            files: 1,
            parent,
            children,
            kind,
            mtime: 0,
        };
        // 0 /
        //   1 proj/ -> 2 Cargo.toml, 3 target/ -> 4 big.BIN
        //   5 node_modules/ -> 6 node_modules/ -> 7 a.js
        let t = Tree {
            nodes: vec![
                node("/", u32::MAX, 0, Kind::Dir, vec![1, 5]),
                node("proj", 0, 0, Kind::Dir, vec![2, 3]),
                node("Cargo.toml", 1, 10, Kind::File, vec![]),
                node("target", 1, 900, Kind::Dir, vec![4]),
                node("big.BIN", 3, 900, Kind::File, vec![]),
                node("node_modules", 0, 50, Kind::Dir, vec![6]),
                node("node_modules", 5, 50, Kind::Dir, vec![7]),
                node("a.js", 6, 50, Kind::File, vec![]),
            ],
            root: PathBuf::from("/"),
        };
        let s = compute(&t);
        assert_eq!(s.largest, vec![4, 7, 2]);
        assert_eq!(s.candidates, vec![(3, "cargo build"), (5, "npm deps")]);
        assert_eq!(s.types[0], ("bin".into(), 900, 1));
        assert_eq!(hint(&t, 1), None);
        assert_eq!(fmt_age(86_400 * 400), "13mo");
    }
}
