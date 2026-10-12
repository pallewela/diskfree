mod scan;

use eframe::egui;
use scan::{fmt_size, Kind, Progress, Sort, Tree};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::thread::JoinHandle;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--cli") {
        cli(args.get(2).map_or(".", String::as_str));
        return Ok(());
    }
    eframe::run_native(
        "diskfree",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 680.0]),
            ..Default::default()
        },
        Box::new(move |_| {
            let mut app = App::default();
            if let Some(root) = args.get(1) {
                app.root_input = root.clone();
                app.start_scan(None);
            }
            Ok(Box::new(app))
        }),
    )
}

fn cli(root: &str) {
    let t = Tree::scan(Path::new(root), &Progress::default());
    let r = &t.nodes[0];
    println!("{:>10}  {}", fmt_size(r.size), r.name);
    for &c in r.children.iter().take(20) {
        let n = &t.nodes[c as usize];
        println!("{:>10}  {}", fmt_size(n.size), n.name);
    }
}

struct Job {
    progress: Arc<Progress>,
    handle: JoinHandle<Tree>,
    target: Option<u32>, // None = full scan, Some(id) = rescan that subtree
}

struct App {
    root_input: String,
    disks: sysinfo::Disks,
    tree: Option<Tree>,
    sort: Sort,
    expanded: Vec<bool>,
    rows: Vec<(u32, u16)>, // (node id, depth) of currently visible rows
    selected: usize,
    job: Option<Job>,
    status: String,
    visible: std::ops::Range<usize>,
    scroll_to: Option<f32>,
    filter: String,
    hit: Vec<bool>, // name matches filter
    anc: Vec<bool>, // has a matching descendant
    confirm_trash: Option<u32>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            root_input: if cfg!(windows) { "C:\\".into() } else { "/".into() },
            disks: sysinfo::Disks::new_with_refreshed_list(),
            tree: None,
            sort: Sort::Size,
            expanded: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            job: None,
            status: String::new(),
            visible: 0..0,
            scroll_to: None,
            filter: String::new(),
            hit: Vec::new(),
            anc: Vec::new(),
            confirm_trash: None,
        }
    }
}

impl App {
    fn start_scan(&mut self, target: Option<u32>) {
        let root = match (target, &self.tree) {
            (Some(id), Some(t)) => t.path(id),
            _ => PathBuf::from(self.root_input.trim()),
        };
        let progress = Arc::new(Progress::default());
        let p = progress.clone();
        let handle = std::thread::spawn(move || Tree::scan(&root, &p));
        self.job = Some(Job { progress, handle, target });
        self.status.clear();
    }

    fn finish_scan(&mut self) {
        let job = self.job.take().unwrap();
        let sub = job.handle.join().expect("scan thread panicked");
        if job.progress.cancel.load(Relaxed) {
            self.status = "Scan cancelled — showing partial results.".into();
        }
        match (job.target, &mut self.tree) {
            (Some(id), Some(t)) => t.graft(id, sub, self.sort),
            _ => {
                let mut t = sub;
                if self.sort != Sort::Size {
                    t.sort_all(self.sort);
                }
                self.tree = Some(t);
                self.expanded.clear();
                self.selected = 0;
            }
        }
        self.after_tree_change();
    }

    /// Call after the arena grew or nodes were removed.
    fn after_tree_change(&mut self) {
        let len = self.tree.as_ref().map_or(0, |t| t.nodes.len());
        self.expanded.resize(len, false);
        self.disks.refresh(true);
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let Some(t) = &self.tree else { return };
        let q = self.filter.trim().to_lowercase();
        self.hit = vec![false; t.nodes.len()];
        self.anc = vec![false; t.nodes.len()];
        if !q.is_empty() {
            // Pre-order over live nodes, then fold matches up to parents in reverse.
            let mut order = Vec::new();
            let mut stack = vec![0u32];
            while let Some(id) = stack.pop() {
                order.push(id);
                stack.extend(&t.nodes[id as usize].children);
            }
            for &id in order.iter().rev() {
                let i = id as usize;
                self.hit[i] = id != 0 && t.nodes[i].name.to_lowercase().contains(&q);
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
        let Some(t) = &self.tree else { return };
        let filtering = !self.filter.trim().is_empty();
        // `inside`: under a matching node, so show its subtree unfiltered.
        let mut stack = vec![(0u32, 0u16, !filtering)];
        while let Some((id, d, inside)) = stack.pop() {
            self.rows.push((id, d));
            let i = id as usize;
            if self.expanded[i] {
                let inside = inside || self.hit[i];
                for &c in t.nodes[i].children.iter().rev() {
                    if inside || self.hit[c as usize] || self.anc[c as usize] {
                        stack.push((c, d + 1, inside));
                    }
                }
            }
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn toggle(&mut self, id: u32, open: bool) {
        self.expanded[id as usize] = open;
        self.rebuild_rows();
    }

    fn trash(&mut self, id: u32) {
        let Some(t) = &mut self.tree else { return };
        let path = t.path(id);
        match trash::delete(&path) {
            Ok(()) => {
                self.status = format!("Moved to Trash: {} ({})", path.display(), fmt_size(t.nodes[id as usize].size));
                t.remove(id, self.sort);
                self.after_tree_change();
            }
            Err(e) => self.status = format!("Trash failed for {}: {e}", path.display()),
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.tree else { return };
        if self.rows.is_empty() || ctx.egui_wants_keyboard_input() || self.confirm_trash.is_some() {
            return;
        }
        let (id, _) = self.rows[self.selected];
        let n = &t.nodes[id as usize];
        let has_kids = !n.children.is_empty();
        let open = self.expanded[id as usize];
        let parent = n.parent;
        let key = |k| ctx.input(|i| i.key_pressed(k));
        let before = self.selected;
        if key(egui::Key::ArrowUp) {
            self.selected = self.selected.saturating_sub(1);
        }
        if key(egui::Key::ArrowDown) {
            self.selected = (self.selected + 1).min(self.rows.len() - 1);
        }
        if key(egui::Key::ArrowRight) && has_kids && !open {
            self.toggle(id, true);
        }
        if key(egui::Key::ArrowLeft) {
            if open {
                self.toggle(id, false);
            } else if let Some(i) = self.rows.iter().position(|&(r, _)| r == parent) {
                self.selected = i;
            }
        }
        if (key(egui::Key::Delete) || key(egui::Key::Backspace)) && id != 0 {
            self.confirm_trash = Some(id);
        }
        if self.selected != before && !self.visible.contains(&self.selected) {
            self.scroll_to = Some(self.selected as f32);
        }
    }

    fn disk_for(&self, path: &Path) -> Option<&sysinfo::Disk> {
        self.disks
            .list()
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let busy = self.job.is_some();
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("vol").selected_text("Volume").show_ui(ui, |ui| {
                for d in self.disks.list() {
                    let mp = d.mount_point().to_string_lossy();
                    let label = format!("{mp}  ({} free)", fmt_size(d.available_space()));
                    if ui.selectable_label(self.root_input == mp, label).clicked() {
                        self.root_input = mp.into_owned();
                    }
                }
            });
            let edit = ui.add(egui::TextEdit::singleline(&mut self.root_input).desired_width(260.0));
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if let Some(j) = &self.job {
                if ui.button("Cancel").clicked() {
                    j.progress.cancel.store(true, Relaxed);
                }
            } else if ui.button("Scan").clicked() || enter {
                self.start_scan(None);
            }
            ui.separator();
            ui.label("Sort:");
            let before = self.sort;
            ui.add_enabled_ui(!busy, |ui| {
                ui.selectable_value(&mut self.sort, Sort::Size, "Size");
                ui.selectable_value(&mut self.sort, Sort::Name, "Name");
                ui.selectable_value(&mut self.sort, Sort::Files, "Files");
            });
            if self.sort != before {
                if let Some(t) = &mut self.tree {
                    t.sort_all(self.sort);
                    self.rebuild_rows();
                }
            }
            ui.separator();
            let f = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter names…").desired_width(160.0));
            if f.changed() {
                self.apply_filter();
            }
        });
        ui.horizontal(|ui| {
            if let Some(j) = &self.job {
                let p = &j.progress;
                ui.label(format!(
                    "Scanning… {} files, {} — {}",
                    p.files.load(Relaxed),
                    fmt_size(p.bytes.load(Relaxed)),
                    p.current.lock().unwrap()
                ));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            if let Some(d) = self.tree.as_ref().and_then(|t| self.disk_for(&t.root)) {
                let (total, free) = (d.total_space(), d.available_space());
                ui.label(format!(
                    "{}: {} used of {}, {} free",
                    d.mount_point().display(),
                    fmt_size(total.saturating_sub(free)),
                    fmt_size(total),
                    fmt_size(free)
                ));
                ui.add(egui::ProgressBar::new(1.0 - free as f32 / total.max(1) as f32).desired_width(120.0));
            }
            ui.label(egui::RichText::new(&self.status).weak());
        });
    }

    fn tree_view(&mut self, ui: &mut egui::Ui) {
        let Some(t) = &self.tree else {
            ui.label("Pick a volume or type a path, then press Scan.");
            return;
        };
        let busy = self.job.is_some();
        let row_h = ui.text_style_height(&egui::TextStyle::Body);
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(i) = self.scroll_to.take() {
            let step = row_h + ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset((i * step - ui.available_height() / 2.0).max(0.0));
        }
        let mut toggle = None;
        let mut rescan = None;
        let out = area.show_rows(ui, row_h, self.rows.len(), |ui, range| {
            for i in range.clone() {
                let (id, depth) = self.rows[i];
                let n = &t.nodes[id as usize];
                let parent_size = t.nodes.get(n.parent as usize).map_or(n.size, |p| p.size);
                let frac = if parent_size == 0 { 0.0 } else { n.size as f32 / parent_size as f32 };
                ui.horizontal(|ui| {
                    ui.add_sized([80.0, row_h], egui::Label::new(egui::RichText::new(fmt_size(n.size)).monospace()));
                    ui.add(egui::ProgressBar::new(frac).desired_width(80.0).desired_height(row_h * 0.7));
                    ui.add_sized([70.0, row_h], egui::Label::new(egui::RichText::new(n.files.to_string()).weak()));
                    ui.add_space(depth as f32 * 14.0);
                    let open = self.expanded[id as usize];
                    let arrow = match (n.kind, n.children.is_empty(), open) {
                        (Kind::Denied, ..) => "🔒",
                        (_, true, _) => " ",
                        (_, false, false) => "▶",
                        (_, false, true) => "▼",
                    };
                    let label = match n.kind {
                        Kind::Dir | Kind::Denied => format!("{arrow} {}/", n.name),
                        Kind::Symlink => format!("{arrow} {} →", n.name),
                        Kind::File => format!("{arrow} {}", n.name),
                    };
                    let mut text = egui::RichText::new(label);
                    if self.hit.get(id as usize) == Some(&true) {
                        text = text.strong().color(ui.visuals().warn_fg_color);
                    }
                    let r = ui.selectable_label(i == self.selected, text);
                    if r.clicked() || r.secondary_clicked() {
                        self.selected = i;
                    }
                    if r.clicked() && !n.children.is_empty() {
                        toggle = Some((id, !open));
                    }
                    r.context_menu(|ui| {
                        let path = t.path(id);
                        if ui.button("Reveal in file manager").clicked() {
                            if let Err(e) = opener::reveal(&path) {
                                self.status = format!("Reveal failed: {e}");
                            }
                        }
                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(path.to_string_lossy().into_owned());
                        }
                        let is_dir = matches!(n.kind, Kind::Dir | Kind::Denied);
                        if ui.add_enabled(is_dir && !busy, egui::Button::new("Rescan")).clicked() {
                            rescan = Some(id);
                        }
                        if ui.add_enabled(id != 0, egui::Button::new("Move to Trash…")).clicked() {
                            self.confirm_trash = Some(id);
                        }
                    });
                    r.on_hover_text(t.path(id).to_string_lossy());
                });
            }
            range
        });
        self.visible = out.inner;
        if let Some((id, open)) = toggle {
            self.toggle(id, open);
        }
        if let Some(id) = rescan {
            self.start_scan(Some(id));
        }
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let (Some(id), Some(t)) = (self.confirm_trash, &self.tree) else { return };
        let n = &t.nodes[id as usize];
        let (path, size, files) = (t.path(id), n.size, n.files);
        let mut answer = None;
        egui::Modal::new("trash".into()).show(ctx, |ui| {
            ui.heading("Move to Trash?");
            ui.label(path.to_string_lossy());
            ui.label(format!("{} in {files} files", fmt_size(size)));
            ui.horizontal(|ui| {
                if ui.button("Move to Trash").clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    answer = Some(false);
                }
            });
        });
        if let Some(yes) = answer {
            self.confirm_trash = None;
            if yes {
                self.trash(id);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if self.job.as_ref().is_some_and(|j| j.handle.is_finished()) {
            self.finish_scan();
        }
        self.keys(ctx);
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.tree_view(ui));
        self.trash_dialog(ctx);
    }
}
