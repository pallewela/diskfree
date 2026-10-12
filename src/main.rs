use diskfree::explorer::Explorer;
use diskfree::insights;
use diskfree::scan::{fmt_size, Kind, Progress, ScanJob, Sort, Tree};
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;

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
                app.start_scan(vec![]);
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
    scan: ScanJob,
    targets: Vec<u32>, // empty = full scan, else rescan these subtrees
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Largest,
    Types,
    Candidates,
}

struct App {
    root_input: String,
    disks: sysinfo::Disks,
    ex: Option<Explorer>,
    sort: Sort,
    selected: usize,   // cursor row
    sel: HashSet<u32>, // selected node ids
    anchor: usize,     // row where shift-click ranges start
    job: Option<Job>,
    status: String,
    visible: std::ops::Range<usize>,
    scroll_to: Option<f32>,
    filter: String,
    confirm_trash: Option<Vec<u32>>,
    show_side: bool,
    side: Side,
    age_days: u64, // dim rows changed within this many days; 0 = off
}

impl Default for App {
    fn default() -> Self {
        Self {
            root_input: if cfg!(windows) { "C:\\".into() } else { "/".into() },
            disks: sysinfo::Disks::new_with_refreshed_list(),
            ex: None,
            sort: Sort::Size,
            selected: 0,
            sel: HashSet::new(),
            anchor: 0,
            job: None,
            status: String::new(),
            visible: 0..0,
            scroll_to: None,
            filter: String::new(),
            confirm_trash: None,
            show_side: true,
            side: Side::Candidates,
            age_days: 0,
        }
    }
}

impl App {
    fn start_scan(&mut self, targets: Vec<u32>) {
        let roots: Vec<PathBuf> = match &self.ex {
            Some(ex) if !targets.is_empty() => targets.iter().map(|&id| ex.tree.path(id)).collect(),
            _ => vec![PathBuf::from(self.root_input.trim())],
        };
        self.job = Some(Job { scan: ScanJob::start(roots), targets });
        self.status.clear();
    }

    fn finish_scan(&mut self) {
        let job = self.job.take().unwrap();
        let cancelled = job.scan.cancelled();
        let subs = job.scan.finish();
        match &mut self.ex {
            // A cancelled rescan would graft partial sizes; keep the old data instead.
            Some(_) if cancelled && !job.targets.is_empty() => self.status = "Rescan cancelled.".into(),
            Some(ex) if !job.targets.is_empty() => ex.graft(job.targets.into_iter().zip(subs).collect()),
            _ => {
                if cancelled {
                    self.status = "Scan cancelled — showing partial results.".into();
                }
                let mut ex = Explorer::new(subs.into_iter().next().unwrap(), self.sort);
                ex.set_filter(&self.filter);
                self.ex = Some(ex);
                self.selected = 0;
                self.sel.clear();
            }
        }
        self.after_tree_change();
    }

    /// Call after the arena grew or nodes were removed.
    fn after_tree_change(&mut self) {
        if let Some(ex) = &self.ex {
            self.sel.retain(|&id| ex.tree.is_live(id));
            self.selected = self.selected.min(ex.rows.len().saturating_sub(1));
        }
        self.disks.refresh(true);
    }

    fn jump(&mut self, id: u32) {
        let Some(ex) = &mut self.ex else { return };
        self.filter.clear();
        if let Some(i) = ex.reveal(id) {
            self.selected = i;
            self.anchor = i;
            self.sel = HashSet::from([id]);
            self.scroll_to = Some(i as f32);
        }
    }

    fn toggle(&mut self, id: u32, open: bool) {
        if let Some(ex) = &mut self.ex {
            ex.toggle(id, open);
            self.selected = self.selected.min(ex.rows.len().saturating_sub(1));
        }
    }

    fn trash_all(&mut self, ids: Vec<u32>) {
        let Some(ex) = &mut self.ex else { return };
        let (mut gone, mut bytes, mut errs) = (Vec::new(), 0, Vec::new());
        for id in ids {
            let path = ex.tree.path(id);
            match trash::delete(&path) {
                Ok(()) => {
                    gone.push(id);
                    bytes += ex.tree.nodes[id as usize].size;
                }
                Err(e) => errs.push(format!("{}: {e}", path.display())),
            }
        }
        ex.remove(&gone);
        self.status = format!("Moved {} item(s), {}, to Trash.", gone.len(), fmt_size(bytes));
        if let Some(e) = errs.first() {
            self.status += &format!(" {} failed — {e}", errs.len());
        }
        self.after_tree_change();
    }

    /// Selected ids for actions: top-level selection, or the cursor row if nothing is selected.
    fn action_ids(&self) -> Vec<u32> {
        let Some(ex) = &self.ex else { return vec![] };
        let ids = ex.tree.top_level(&self.sel);
        if ids.is_empty() { ex.rows.get(self.selected).map(|r| r.0).into_iter().collect() } else { ids }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let Some(ex) = &self.ex else { return };
        if ex.rows.is_empty() || ctx.egui_wants_keyboard_input() || self.confirm_trash.is_some() {
            return;
        }
        let (id, _) = ex.rows[self.selected];
        let n = &ex.tree.nodes[id as usize];
        let has_kids = !n.children.is_empty();
        let open = ex.expanded[id as usize];
        let parent = n.parent;
        let key = |k| ctx.input(|i| i.key_pressed(k));
        let shift = ctx.input(|i| i.modifiers.shift);
        let before = self.selected;
        let last = ex.rows.len() - 1;
        if key(egui::Key::ArrowUp) {
            self.selected = self.selected.saturating_sub(1);
        }
        if key(egui::Key::ArrowDown) {
            self.selected = (self.selected + 1).min(last);
        }
        if key(egui::Key::ArrowRight) && has_kids && !open {
            self.toggle(id, true);
        }
        if key(egui::Key::ArrowLeft) {
            if open {
                self.toggle(id, false);
            } else if let Some(i) = self.ex.as_ref().unwrap().rows.iter().position(|&(r, _)| r == parent) {
                self.selected = i;
            }
        }
        let rows = &self.ex.as_ref().unwrap().rows;
        if self.selected != before {
            let nid = rows[self.selected].0;
            if shift {
                self.sel.insert(nid);
            } else {
                self.sel = HashSet::from([nid]);
                self.anchor = self.selected;
            }
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
            self.sel = rows.iter().map(|r| r.0).collect();
        }
        if key(egui::Key::Escape) {
            self.sel.clear();
        }
        if key(egui::Key::Delete) || key(egui::Key::Backspace) {
            let ids: Vec<u32> = self.action_ids().into_iter().filter(|&id| id != 0).collect();
            if !ids.is_empty() {
                self.confirm_trash = Some(ids);
            }
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
        ui.horizontal_wrapped(|ui| {
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
                    j.scan.cancel();
                }
            } else if ui.button("Scan").clicked() || enter {
                self.start_scan(vec![]);
            }
            ui.separator();
            ui.label("Sort:");
            ui.add_enabled_ui(!busy, |ui| {
                ui.selectable_value(&mut self.sort, Sort::Size, "Size");
                ui.selectable_value(&mut self.sort, Sort::Name, "Name");
                ui.selectable_value(&mut self.sort, Sort::Files, "Files");
            });
            if let Some(ex) = &mut self.ex {
                ex.set_sort(self.sort);
            }
            ui.separator();
            let f = ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("filter names…").desired_width(160.0));
            if f.changed() {
                if let Some(ex) = &mut self.ex {
                    ex.set_filter(&self.filter);
                }
            }
            let age = |d: u64| if d == 0 { "Dim recent: off".to_string() } else { format!("Dim changed < {d}d") };
            egui::ComboBox::from_id_salt("age").selected_text(age(self.age_days)).show_ui(ui, |ui| {
                for d in [0, 7, 30, 90, 365] {
                    ui.selectable_value(&mut self.age_days, d, age(d));
                }
            });
            ui.toggle_value(&mut self.show_side, "Insights");
        });
        ui.horizontal(|ui| {
            if let Some(j) = &self.job {
                let p = &j.scan.progress;
                ui.label(format!(
                    "Scanning… {} files, {} — {}",
                    p.files.load(Relaxed),
                    fmt_size(p.bytes.load(Relaxed)),
                    p.current.lock().unwrap()
                ));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            if let Some(d) = self.ex.as_ref().and_then(|ex| self.disk_for(&ex.tree.root)) {
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
            if let Some(ex) = &self.ex {
                let ids = ex.tree.top_level(&self.sel);
                if !ids.is_empty() {
                    let size: u64 = ids.iter().map(|&id| ex.tree.nodes[id as usize].size).sum();
                    ui.separator();
                    ui.label(egui::RichText::new(format!("{} selected, {}", ids.len(), fmt_size(size))).strong());
                }
            }
            ui.label(egui::RichText::new(&self.status).weak());
        });
    }

    fn tree_view(&mut self, ui: &mut egui::Ui) {
        let Some(ex) = &self.ex else {
            ui.label("Pick a volume or type a path, then press Scan.");
            return;
        };
        let t = &ex.tree;
        let busy = self.job.is_some();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let recent = now.saturating_sub(self.age_days * 86_400);
        let row_h = ui.text_style_height(&egui::TextStyle::Body);
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if let Some(i) = self.scroll_to.take() {
            let step = row_h + ui.spacing().item_spacing.y;
            area = area.vertical_scroll_offset((i * step - ui.available_height() / 2.0).max(0.0));
        }
        let mut toggle = None;
        let mut rescan = Vec::new();
        let out = area.show_rows(ui, row_h, ex.rows.len(), |ui, range| {
            for i in range.clone() {
                let (id, depth) = ex.rows[i];
                let n = &t.nodes[id as usize];
                let parent_size = t.nodes.get(n.parent as usize).map_or(n.size, |p| p.size);
                let frac = if parent_size == 0 { 0.0 } else { n.size as f32 / parent_size as f32 };
                ui.horizontal(|ui| {
                    ui.add_sized([80.0, row_h], egui::Label::new(egui::RichText::new(fmt_size(n.size)).monospace()));
                    ui.add(egui::ProgressBar::new(frac).desired_width(80.0).desired_height(row_h * 0.7));
                    ui.add_sized([70.0, row_h], egui::Label::new(egui::RichText::new(n.files.to_string()).weak()));
                    let age = if n.mtime == 0 { String::new() } else { insights::fmt_age(now.saturating_sub(n.mtime)) };
                    ui.add_sized([45.0, row_h], egui::Label::new(egui::RichText::new(age).weak()));
                    ui.add_space(depth as f32 * 14.0);
                    let open = ex.expanded[id as usize];
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
                    if ex.hit[id as usize] {
                        text = text.strong().color(ui.visuals().warn_fg_color);
                    } else if self.age_days > 0 && n.mtime > recent {
                        text = text.color(ui.visuals().weak_text_color());
                    }
                    let is_sel = self.sel.contains(&id);
                    let r = ui.selectable_label(is_sel, text);
                    if r.clicked() {
                        let m = ui.input(|i| i.modifiers);
                        if m.shift {
                            let a = self.anchor.min(ex.rows.len() - 1);
                            self.sel = ex.rows[a.min(i)..=a.max(i)].iter().map(|r| r.0).collect();
                        } else if m.command {
                            if !self.sel.remove(&id) {
                                self.sel.insert(id);
                            }
                            self.anchor = i;
                        } else {
                            self.sel = HashSet::from([id]);
                            self.anchor = i;
                            if !n.children.is_empty() {
                                toggle = Some((id, !open));
                            }
                        }
                        self.selected = i;
                    }
                    if r.secondary_clicked() && !is_sel {
                        self.sel = HashSet::from([id]);
                        self.anchor = i;
                        self.selected = i;
                    }
                    r.context_menu(|ui| {
                        let ids = t.top_level(&self.sel);
                        let many = if ids.len() > 1 { format!(" ({} items)", ids.len()) } else { String::new() };
                        if ui.button(format!("Reveal in file manager{many}")).clicked() {
                            for &id in &ids {
                                if let Err(e) = opener::reveal(t.path(id)) {
                                    self.status = format!("Reveal failed: {e}");
                                }
                            }
                        }
                        if ui.button(format!("Copy path{many}")).clicked() {
                            let paths: Vec<String> = ids.iter().map(|&id| t.path(id).to_string_lossy().into_owned()).collect();
                            ui.ctx().copy_text(paths.join("\n"));
                        }
                        let dirs: Vec<u32> =
                            ids.iter().copied().filter(|&id| matches!(t.nodes[id as usize].kind, Kind::Dir | Kind::Denied)).collect();
                        let label = if dirs.len() > 1 { format!("Rescan ({} folders)", dirs.len()) } else { "Rescan".into() };
                        if ui.add_enabled(!dirs.is_empty() && !busy, egui::Button::new(label)).clicked() {
                            rescan = dirs;
                        }
                        let doomed: Vec<u32> = ids.into_iter().filter(|&id| id != 0).collect();
                        let label = if doomed.len() > 1 { format!("Move {} items to Trash…", doomed.len()) } else { "Move to Trash…".into() };
                        if ui.add_enabled(!doomed.is_empty(), egui::Button::new(label)).clicked() {
                            self.confirm_trash = Some(doomed);
                        }
                    });
                    r.on_hover_text(t.path(id).to_string_lossy());
                    if let Some(h) = insights::hint(t, id) {
                        ui.label(
                            egui::RichText::new(format!(" {h} "))
                                .small()
                                .color(egui::Color32::WHITE)
                                .background_color(egui::Color32::from_rgb(60, 110, 190)),
                        );
                    }
                });
            }
            range
        });
        self.visible = out.inner;
        if let Some((id, open)) = toggle {
            self.toggle(id, open);
        }
        if !rescan.is_empty() {
            self.start_scan(rescan);
        }
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let Some(ex) = &self.ex else {
            ui.label("Scan first.");
            return;
        };
        let (t, stats) = (&ex.tree, &ex.stats);
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.side, Side::Candidates, "Cleanup hints");
            ui.selectable_value(&mut self.side, Side::Largest, "Largest files");
            ui.selectable_value(&mut self.side, Side::Types, "File types");
        });
        ui.separator();
        let rel = |id: u32| t.path(id).strip_prefix(&t.root).map_or(String::new(), |p| p.display().to_string());
        let row = |size: u64, rest: String| egui::RichText::new(format!("{:>10}  {rest}", fmt_size(size))).monospace();
        let (mut jump, mut filter) = (None, None);
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match self.side {
            Side::Candidates => {
                let total: u64 = stats.candidates.iter().map(|c| t.nodes[c.0 as usize].size).sum();
                ui.horizontal(|ui| {
                    ui.label(format!("{} found, {}", stats.candidates.len(), fmt_size(total)));
                    if ui.button("Select all").on_hover_text("Then press Delete to trash them").clicked() {
                        self.sel = stats.candidates.iter().map(|c| c.0).collect();
                    }
                });
                for &(id, h) in &stats.candidates {
                    let r = ui.selectable_label(self.sel.contains(&id), row(t.nodes[id as usize].size, format!("{}  [{h}]", rel(id))));
                    if r.clicked() {
                        jump = Some(id);
                    }
                }
            }
            Side::Largest => {
                ui.label(format!("Top {} files", insights::TOP_FILES));
                for &id in &stats.largest {
                    if ui.selectable_label(self.sel.contains(&id), row(t.nodes[id as usize].size, rel(id))).clicked() {
                        jump = Some(id);
                    }
                }
            }
            Side::Types => {
                for (ext, bytes, files) in &stats.types {
                    let r = ui.selectable_label(false, row(*bytes, format!(".{ext}  ({files} files)")));
                    if r.on_hover_text("Filter the tree by this extension").clicked() && ext != "(none)" {
                        filter = Some(format!(".{ext}"));
                    }
                }
            }
        });
        if let Some(id) = jump {
            self.jump(id);
        }
        if let (Some(f), Some(ex)) = (filter, &mut self.ex) {
            ex.set_filter(&f);
            self.filter = f;
        }
    }

    fn trash_dialog(&mut self, ctx: &egui::Context) {
        let (Some(ids), Some(ex)) = (&self.confirm_trash, &self.ex) else { return };
        let t = &ex.tree;
        let size: u64 = ids.iter().map(|&id| t.nodes[id as usize].size).sum();
        let files: u64 = ids.iter().map(|&id| t.nodes[id as usize].files).sum();
        let mut answer = None;
        egui::Modal::new("trash".into()).show(ctx, |ui| {
            ui.heading(format!("Move {} item(s) to Trash?", ids.len()));
            const SHOW: usize = 10;
            for &id in ids.iter().take(SHOW) {
                ui.label(t.path(id).to_string_lossy());
            }
            if ids.len() > SHOW {
                ui.label(format!("…and {} more", ids.len() - SHOW));
            }
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
            let ids = self.confirm_trash.take().unwrap();
            if yes {
                self.trash_all(ids);
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if self.job.as_ref().is_some_and(|j| j.scan.is_done()) {
            self.finish_scan();
        }
        self.keys(ctx);
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));
        if self.show_side {
            egui::Panel::right("side").resizable(true).default_size(380.0).show(ui, |ui| self.side_panel(ui));
        }
        egui::CentralPanel::default().show(ui, |ui| self.tree_view(ui));
        self.trash_dialog(ctx);
    }
}
