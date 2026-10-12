//! UI-agnostic browsing state shared by the egui app and the native macOS app (via `ffi`).

use crate::insights::{self, Stats};
use crate::scan::{Sort, Tree};

pub struct Explorer {
    pub tree: Tree,
    pub sort: Sort,
    pub expanded: Vec<bool>,
    pub rows: Vec<(u32, u16)>, // (node id, depth) of currently visible rows
    pub hit: Vec<bool>,        // name matches filter
    pub anc: Vec<bool>,        // has a matching descendant
    pub stats: Stats,
    query: String,
}

impl Explorer {
    pub fn new(mut tree: Tree, sort: Sort) -> Self {
        if sort != Sort::Size {
            tree.sort_all(sort);
        }
        let mut ex = Explorer {
            tree,
            sort,
            expanded: Vec::new(),
            rows: Vec::new(),
            hit: Vec::new(),
            anc: Vec::new(),
            stats: Stats::default(),
            query: String::new(),
        };
        ex.refresh();
        ex
    }

    /// Recompute everything derived from the tree. Call after the arena changed.
    fn refresh(&mut self) {
        self.expanded.resize(self.tree.nodes.len(), false);
        self.apply_filter();
        self.stats = insights::compute(&self.tree);
    }

    pub fn set_sort(&mut self, by: Sort) {
        if by != self.sort {
            self.sort = by;
            self.tree.sort_all(by);
            self.rebuild_rows();
        }
    }

    pub fn filter(&self) -> &str {
        &self.query
    }

    pub fn set_filter(&mut self, q: &str) {
        self.query = q.trim().to_lowercase();
        self.apply_filter();
    }

    pub fn toggle(&mut self, id: u32, open: bool) {
        self.expanded[id as usize] = open;
        self.rebuild_rows();
    }

    /// Expand down to `id` (clearing any filter) and return its row index.
    pub fn reveal(&mut self, id: u32) -> Option<usize> {
        let mut p = self.tree.nodes[id as usize].parent;
        while p != u32::MAX {
            self.expanded[p as usize] = true;
            p = self.tree.nodes[p as usize].parent;
        }
        self.query.clear();
        self.apply_filter();
        self.rows.iter().position(|r| r.0 == id)
    }

    /// Drop nodes that were deleted from disk.
    pub fn remove(&mut self, ids: &[u32]) {
        for &id in ids {
            if self.tree.is_live(id) && id != 0 {
                self.tree.remove(id, self.sort);
            }
        }
        self.refresh();
    }

    /// Replace subtrees with fresh scans of the same paths.
    pub fn graft(&mut self, subs: Vec<(u32, Tree)>) {
        for (id, sub) in subs {
            if self.tree.is_live(id) {
                self.tree.graft(id, sub, self.sort);
            }
        }
        self.refresh();
    }

    fn apply_filter(&mut self) {
        let n = self.tree.nodes.len();
        self.hit = vec![false; n];
        self.anc = vec![false; n];
        if !self.query.is_empty() {
            // Pre-order over live nodes, then fold matches up to parents in reverse.
            let t = &self.tree;
            let mut order = Vec::new();
            let mut stack = vec![0u32];
            while let Some(id) = stack.pop() {
                order.push(id);
                stack.extend(&t.nodes[id as usize].children);
            }
            for &id in order.iter().rev() {
                let i = id as usize;
                self.hit[i] = id != 0 && t.nodes[i].name.to_lowercase().contains(&self.query);
                if (self.hit[i] || self.anc[i]) && id != 0 {
                    let p = t.nodes[i].parent as usize;
                    self.anc[p] = true;
                    self.expanded[p] = true;
                }
            }
        }
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let filtering = !self.query.is_empty();
        // `inside`: under a matching node, so show its subtree unfiltered.
        let mut stack = vec![(0u32, 0u16, !filtering)];
        while let Some((id, d, inside)) = stack.pop() {
            self.rows.push((id, d));
            let i = id as usize;
            if self.expanded[i] {
                let inside = inside || self.hit[i];
                for &c in self.tree.nodes[i].children.iter().rev() {
                    if inside || self.hit[c as usize] || self.anc[c as usize] {
                        stack.push((c, d + 1, inside));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{Kind, Node};
    use std::path::PathBuf;

    fn tree() -> Tree {
        // 0 / -> 1 a/ -> 2 x.log ; 0 -> 3 b/ -> 4 y.txt
        let node = |name: &str, parent, size, kind, children| Node { name: name.into(), size, files: 1, parent, children, kind, mtime: 0 };
        Tree {
            nodes: vec![
                node("/", u32::MAX, 30, Kind::Dir, vec![1, 3]),
                node("a", 0, 20, Kind::Dir, vec![2]),
                node("x.log", 1, 20, Kind::File, vec![]),
                node("b", 0, 10, Kind::Dir, vec![4]),
                node("y.txt", 3, 10, Kind::File, vec![]),
            ],
            root: PathBuf::from("/"),
        }
    }

    #[test]
    fn rows_filter_reveal_remove() {
        let mut ex = Explorer::new(tree(), Sort::Size);
        assert_eq!(ex.rows, vec![(0, 0)], "starts collapsed");
        ex.set_filter("Y.T");
        assert_eq!(ex.rows, vec![(0, 0), (3, 1), (4, 2)], "only the match and its ancestors");
        assert_eq!(ex.reveal(2), Some(2));
        assert_eq!(ex.filter(), "");
        assert_eq!(ex.rows, vec![(0, 0), (1, 1), (2, 2), (3, 1), (4, 2)]);
        ex.remove(&[1]);
        assert_eq!(ex.tree.nodes[0].size, 10);
        assert_eq!(ex.rows, vec![(0, 0), (3, 1), (4, 2)]);
        ex.set_sort(Sort::Name);
        assert_eq!(ex.tree.nodes[0].children, vec![3]);
    }
}
