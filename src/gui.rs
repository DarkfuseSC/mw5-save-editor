//! MW5 Save Editor GUI (egui). Single exe, no runtime, no network.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui;
use mw5save::campaign::{ActorInfo, Campaign, CoyoteEntry, FactionInfo, MechInfo, PilotInfo, SKILLS};
use mw5save::diagnose::{diagnose, Report, Severity};
use mw5save::repair::{repair, RepairOutcome};
use std::path::{Path, PathBuf};

const APP_NAME: &str = "MW5 Save Editor";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width: w, height: h })
}

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 720.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title(format!("{} {}", APP_NAME, VERSION))
        .with_taskbar(true);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(APP_NAME, opts, Box::new(|cc| {
        cc.egui_ctx.set_pixels_per_point(1.15);
        Ok(Box::new(App::default()))
    }))
}

#[derive(PartialEq, Clone, Copy)]
enum Tab { Overview, Mechs, Pilots, Factions, Coyote, Diagnose, Browser }

#[derive(Clone)]
struct CoyoteEdit { string_val: String, float_val: String, bool_val: bool, int_val: String }

/// Friendly view over the Coyote counters that represent things you own.
/// (id, label, is_flag). Ids missing from a save are simply not shown.
const COYOTE_ASSETS: &[(&str, &str, &str, bool)] = &[
    ("Aerospace fighters", "ASF_light", "Light fighters", false),
    ("Aerospace fighters", "ASF_med", "Medium fighters", false),
    ("Aerospace fighters", "ASF_heavy", "Heavy fighters", false),
    ("Aerospace fighters", "ASF_Arty_Light", "Light artillery strike unlocked", true),
    ("Aerospace fighters", "ASF_Arty_Med", "Medium artillery strike unlocked", true),
    ("Aerospace fighters", "ASF_Arty_HVY", "Heavy artillery strike unlocked", true),
    ("Tanks", "tank_amount_scorpion", "Scorpion", false),
    ("Tanks", "tank_amount_edgar", "Edgar", false),
    ("Tanks", "tank_amount_demolisher", "Demolisher", false),
    ("Tanks", "tank_amount_manticore", "Manticore", false),
    ("Tanks", "tank_amount_srm", "SRM carrier", false),
    ("Tanks", "tank_amount_lrm", "LRM carrier", false),
    ("Ravager turrets (dropship)", "Coy_ravager1", "Ravager turret 1", false),
    ("Ravager turrets (dropship)", "Coy_ravager2", "Ravager turret 2", false),
    ("Ravager turrets (dropship)", "Coy_ravager3", "Ravager turret 3", false),
];

struct Loaded {
    path: PathBuf,
    original: Vec<u8>,
    campaign: Option<Campaign>,
    report: Report,
    repair_outcome: Option<RepairOutcome>,
    repaired_bytes: Option<Vec<u8>>,
    dirty: bool,
    // edit buffers
    cbills: String,
    reputation: String,
    days: String,
    mechs: Vec<MechInfo>,
    pilots: Vec<PilotInfo>,
    factions: Vec<FactionInfo>,
    mech_name_edit: Vec<String>,
    pilot_edit: Vec<PilotEdit>,
    faction_edit: Vec<String>,
    actors: Vec<ActorInfo>,
    coyote: Vec<CoyoteEntry>,
    coyote_edit: Vec<CoyoteEdit>,
    coyote_filter: String,
}

#[derive(Clone)]
struct PilotEdit { callsign: String, salary: String, skills: Vec<String> }

struct SaveEntry { path: PathBuf, campaign_name: String, size: u64, modified: String }

struct App {
    tab: Tab,
    loaded: Option<Loaded>,
    status: String,
    saves: Vec<SaveEntry>,
    saves_root: Option<PathBuf>,
    log: Vec<String>,
}

impl Default for App {
    fn default() -> Self {
        let mut app = App { tab: Tab::Browser, loaded: None, status: String::new(), saves: Vec::new(), saves_root: None, log: Vec::new() };
        app.saves_root = default_saves_root();
        app.scan_saves();
        app.status = format!("{} {}. Open a save or pick one from the browser.", APP_NAME, VERSION);
        app
    }
}

fn default_saves_root() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    let p = PathBuf::from(local).join("MW5Mercs").join("Saved").join("SaveGames");
    if p.is_dir() { Some(p) } else { None }
}

fn fmt_time(t: std::time::SystemTime) -> String {
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = mw5save::campaign::civil_from_days(days + 719_162);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, rem / 3600, (rem % 3600) / 60)
}

fn campaign_label(dir: &Path) -> String {
    // Campaign.json is small; pull a "Name" or "CompanyName" field if present.
    let Ok(txt) = std::fs::read_to_string(dir.join("Campaign.json")) else { return dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default() };
    for key in ["\"CompanyName\"", "\"Name\"", "\"CampaignName\""] {
        if let Some(i) = txt.find(key) {
            let rest = &txt[i + key.len()..];
            if let Some(q1) = rest.find('"') {
                let rest = &rest[q1 + 1..];
                if let Some(q2) = rest.find('"') {
                    return rest[..q2].to_string();
                }
            }
        }
    }
    dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

impl App {
    fn scan_saves(&mut self) {
        self.saves.clear();
        let Some(root) = &self.saves_root else { return };
        let Ok(dirs) = std::fs::read_dir(root) else { return };
        let mut entries = Vec::new();
        for d in dirs.flatten() {
            let dp = d.path();
            if !dp.is_dir() { continue; }
            let label = campaign_label(&dp);
            if let Ok(files) = std::fs::read_dir(&dp) {
                for f in files.flatten() {
                    let fp = f.path();
                    if fp.extension().map(|e| e == "sav").unwrap_or(false) {
                        let md = f.metadata().ok();
                        entries.push(SaveEntry {
                            path: fp,
                            campaign_name: label.clone(),
                            size: md.as_ref().map(|m| m.len()).unwrap_or(0),
                            modified: md.and_then(|m| m.modified().ok()).map(fmt_time).unwrap_or_default(),
                        });
                    }
                }
            }
        }
        entries.sort_by(|a, b| b.modified.cmp(&a.modified));
        self.saves = entries;
    }

    fn open(&mut self, path: PathBuf) {
        match std::fs::read(&path) {
            Err(e) => self.status = format!("Cannot read {}: {}", path.display(), e),
            Ok(data) => {
                let report = diagnose(&data);
                let campaign = if report.fatal() { None } else { Campaign::open(&data).ok() };
                let mut l = Loaded {
                    path: path.clone(), original: data, campaign, report, repair_outcome: None, repaired_bytes: None, dirty: false,
                    cbills: String::new(), reputation: String::new(), days: String::new(),
                    mechs: Vec::new(), pilots: Vec::new(), factions: Vec::new(),
                    mech_name_edit: Vec::new(), pilot_edit: Vec::new(), faction_edit: Vec::new(),
                    actors: Vec::new(), coyote: Vec::new(), coyote_edit: Vec::new(), coyote_filter: String::new(),
                };
                l.refresh();
                let verdict = if l.report.fatal() { "DAMAGED" } else if l.report.suspect() { "suspect" } else { "clean" };
                self.status = format!("Opened {} ({} bytes, {})", path.file_name().unwrap_or_default().to_string_lossy(), l.original.len(), verdict);
                self.tab = if l.report.fatal() { Tab::Diagnose } else { Tab::Overview };
                self.log.push(self.status.clone());
                self.loaded = Some(l);
            }
        }
    }

    fn save_as(&mut self, dest: PathBuf) {
        let Some(l) = &mut self.loaded else { return };
        let bytes = if let Some(c) = &mut l.campaign {
            c.to_bytes()
        } else if let Some(r) = &l.repaired_bytes {
            r.clone()
        } else {
            self.status = "Nothing to save: file is damaged and no repair was applied.".into();
            return;
        };
        // Backup beside the destination if it exists.
        if dest.exists() {
            let stamp = fmt_time(std::time::SystemTime::now()).replace([':', ' '], "-");
            let bak = dest.with_extension(format!("sav.{}.bak", stamp));
            if let Err(e) = std::fs::copy(&dest, &bak) {
                self.status = format!("Backup failed, not saving: {}", e);
                return;
            }
            self.log.push(format!("Backup written: {}", bak.display()));
        }
        let tmp = dest.with_extension("sav.tmp");
        if let Err(e) = std::fs::write(&tmp, &bytes).and_then(|_| std::fs::rename(&tmp, &dest)) {
            self.status = format!("Save failed: {}", e);
            return;
        }
        // Verify what we wrote.
        let check = diagnose(&bytes);
        l.dirty = false;
        self.status = format!("Saved {} ({} bytes, verify: {})", dest.display(), bytes.len(),
            if check.fatal() { "DAMAGED" } else { "clean" });
        self.log.push(self.status.clone());
    }
}

impl Loaded {
    fn refresh(&mut self) {
        let Some(c) = &mut self.campaign else { return };
        self.cbills = c.cbills().map(|v| v.to_string()).unwrap_or_default();
        self.reputation = c.reputation().map(|v| v.to_string()).unwrap_or_default();
        self.days = c.days_elapsed().map(|v| v.to_string()).unwrap_or_default();
        self.mechs = c.mechs();
        self.mech_name_edit = self.mechs.iter().map(|m| m.custom_name.clone()).collect();
        self.pilots = c.pilots();
        self.pilot_edit = self.pilots.iter().map(|p| PilotEdit {
            callsign: p.callsign.clone(), salary: p.salary.to_string(),
            skills: p.skills.iter().map(|s| s.to_string()).collect(),
        }).collect();
        self.factions = c.factions();
        self.faction_edit = self.factions.iter().map(|f| f.standing.to_string()).collect();
        self.actors = c.actors();
        self.coyote = c.coyote_entries();
        self.coyote_edit = self.coyote.iter().map(|e| CoyoteEdit {
            string_val: e.string_val.clone(), float_val: e.float_val.to_string(), bool_val: e.bool_val, int_val: e.int_val.to_string(),
        }).collect();
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drag and drop
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.open(p);
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open...").clicked() {
                    let mut dlg = rfd::FileDialog::new().add_filter("MW5 save", &["sav"]);
                    if let Some(r) = &self.saves_root { dlg = dlg.set_directory(r); }
                    if let Some(p) = dlg.pick_file() { self.open(p); }
                }
                let has = self.loaded.is_some();
                let dirty = self.loaded.as_ref().map(|l| l.dirty).unwrap_or(false);
                if ui.add_enabled(has && dirty, egui::Button::new("Save (backup first)")).clicked() {
                    let p = self.loaded.as_ref().unwrap().path.clone();
                    self.save_as(p);
                }
                if ui.add_enabled(has, egui::Button::new("Save As...")).clicked() {
                    let cur = self.loaded.as_ref().unwrap().path.clone();
                    let mut dlg = rfd::FileDialog::new().add_filter("MW5 save", &["sav"]);
                    if let Some(d) = cur.parent() { dlg = dlg.set_directory(d); }
                    if let Some(n) = cur.file_name() { dlg = dlg.set_file_name(n.to_string_lossy()); }
                    if let Some(p) = dlg.save_file() { self.save_as(p); }
                }
                if ui.button("Rescan saves").clicked() { self.scan_saves(); }
                ui.separator();
                ui.selectable_value(&mut self.tab, Tab::Browser, "Browser");
                let enabled = self.loaded.as_ref().map(|l| l.campaign.is_some()).unwrap_or(false);
                ui.add_enabled_ui(enabled, |ui| {
                    ui.selectable_value(&mut self.tab, Tab::Overview, "Overview");
                    ui.selectable_value(&mut self.tab, Tab::Mechs, "Mechs");
                    ui.selectable_value(&mut self.tab, Tab::Pilots, "Pilots");
                    ui.selectable_value(&mut self.tab, Tab::Factions, "Factions");
                    let has_coyote = self.loaded.as_ref().map(|l| !l.coyote.is_empty()).unwrap_or(false);
                    ui.selectable_value(&mut self.tab, Tab::Coyote, if has_coyote { "Coyote's Mission Pack" } else { "Mods" });
                });
                ui.add_enabled_ui(has, |ui| { ui.selectable_value(&mut self.tab, Tab::Diagnose, "Diagnose / Repair"); });
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if let Some(l) = &self.loaded {
                    let (txt, col) = if l.report.fatal() { ("DAMAGED", egui::Color32::from_rgb(220, 60, 60)) }
                        else if l.report.suspect() { ("SUSPECT", egui::Color32::from_rgb(230, 170, 40)) }
                        else { ("CLEAN", egui::Color32::from_rgb(70, 180, 90)) };
                    ui.colored_label(col, txt);
                    if l.dirty { ui.colored_label(egui::Color32::from_rgb(230, 170, 40), "unsaved changes"); }
                    ui.separator();
                }
                ui.label(&self.status);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.tab {
                Tab::Browser => self.ui_browser(ui),
                Tab::Overview => self.ui_overview(ui),
                Tab::Mechs => self.ui_mechs(ui),
                Tab::Pilots => self.ui_pilots(ui),
                Tab::Factions => self.ui_factions(ui),
                Tab::Coyote => self.ui_coyote(ui),
                Tab::Diagnose => self.ui_diagnose(ui),
            }
        });
    }
}

impl App {
    fn ui_browser(&mut self, ui: &mut egui::Ui) {
        ui.heading("Save files");
        match &self.saves_root {
            Some(r) => { ui.label(format!("Folder: {}", r.display())); }
            None => { ui.label("SaveGames folder not found under %LOCALAPPDATA%\\MW5Mercs. Use Open... or drag a .sav here."); }
        }
        ui.add_space(6.0);
        let mut to_open = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("saves").striped(true).num_columns(4).spacing([16.0, 4.0]).show(ui, |ui| {
                ui.strong("Campaign"); ui.strong("Save"); ui.strong("Modified"); ui.strong("Size"); ui.end_row();
                for s in &self.saves {
                    ui.label(&s.campaign_name);
                    if ui.link(s.path.file_name().unwrap_or_default().to_string_lossy()).clicked() { to_open = Some(s.path.clone()); }
                    ui.label(&s.modified);
                    ui.label(format!("{:.1} MB", s.size as f64 / 1_048_576.0));
                    ui.end_row();
                }
            });
        });
        if let Some(p) = to_open { self.open(p); }
    }

    fn ui_overview(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        let Some(c) = &mut l.campaign else { return };
        ui.heading(l.path.file_name().unwrap_or_default().to_string_lossy());
        ui.label(format!("In-game date: {}", c.date_text()));
        ui.add_space(8.0);
        egui::Grid::new("overview").num_columns(3).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("C-Bills");
            ui.add(egui::TextEdit::singleline(&mut l.cbills).desired_width(160.0));
            if ui.button("Apply").clicked() {
                if let Ok(v) = l.cbills.trim().replace([',', '_'], "").parse::<i64>() {
                    if c.set_cbills(v) { l.dirty = true; self.status = format!("C-Bills set to {}", v); }
                } else { self.status = "C-Bills must be a whole number".into(); }
            }
            ui.end_row();
            ui.label("Reputation");
            ui.add(egui::TextEdit::singleline(&mut l.reputation).desired_width(160.0));
            if ui.button("Apply").clicked() {
                if let Ok(v) = l.reputation.trim().parse::<i32>() {
                    if c.set_reputation(v) { l.dirty = true; self.status = format!("Reputation set to {}", v); }
                } else { self.status = "Reputation must be a whole number".into(); }
            }
            ui.end_row();
            ui.label("Days elapsed");
            ui.add(egui::TextEdit::singleline(&mut l.days).desired_width(160.0));
            if ui.button("Apply").clicked() {
                if let Ok(v) = l.days.trim().parse::<i32>() {
                    if v >= 0 && c.set_days_elapsed(v) { l.dirty = true; self.status = format!("Days elapsed set to {}", v); }
                } else { self.status = "Days must be a whole number".into(); }
            }
            ui.end_row();
        });
        ui.add_space(12.0);
        let owned = l.mechs.iter().filter(|m| m.is_owned()).count();
        ui.label(format!("{} mechs in active bay, {} total wrapper records, {} pilots, {} factions",
            owned, l.mechs.len(), l.pilots.len(), l.factions.len()));
    }

    fn ui_mechs(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        let Some(c) = &mut l.campaign else { return };
        ui.heading("Mechs");
        ui.label("Custom name is editable. Chassis and loadout editing come after YAML asset validation is in place.");
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("mechs").striped(true).num_columns(5).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.strong("#"); ui.strong("Where"); ui.strong("Chassis"); ui.strong("Custom name"); ui.strong(""); ui.end_row();
                for (row, m) in l.mechs.iter().enumerate() {
                    ui.label(m.index.to_string());
                    ui.label(m.category());
                    ui.label(&m.chassis);
                    ui.add(egui::TextEdit::singleline(&mut l.mech_name_edit[row]).desired_width(200.0));
                    if ui.button("Apply").clicked() && c.set_mech_custom_name(m.index, &l.mech_name_edit[row]) {
                        l.dirty = true;
                        self.status = format!("Mech {} renamed", m.index);
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn ui_pilots(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        let Some(c) = &mut l.campaign else { return };
        ui.heading("Pilots");
        ui.label("Skill values are TotalExp. Commander row is marked; its slot is never changed.");
        egui::ScrollArea::both().show(ui, |ui| {
            egui::Grid::new("pilots").striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.strong("Callsign"); ui.strong("Name"); ui.strong("Salary");
                for s in SKILLS { ui.strong(*s); }
                ui.strong(""); ui.end_row();
                for (row, p) in l.pilots.iter().enumerate() {
                    let e = &mut l.pilot_edit[row];
                    ui.add(egui::TextEdit::singleline(&mut e.callsign).desired_width(110.0));
                    ui.label(if p.is_commander { format!("{} (commander)", p.full_name) } else { p.full_name.clone() });
                    ui.add(egui::TextEdit::singleline(&mut e.salary).desired_width(70.0));
                    for s in e.skills.iter_mut() { ui.add(egui::TextEdit::singleline(s).desired_width(55.0)); }
                    if ui.button("Apply").clicked() {
                        let mut changed = false;
                        if e.callsign != p.callsign { changed |= c.set_pilot_callsign(p.index, &e.callsign); }
                        if let Ok(v) = e.salary.trim().parse::<i32>() { if v != p.salary { changed |= c.set_pilot_salary(p.index, v); } }
                        for (k, s) in SKILLS.iter().enumerate() {
                            if let Ok(v) = e.skills[k].trim().parse::<i32>() { if v != p.skills[k] { changed |= c.set_pilot_skill(p.index, s, v); } }
                        }
                        if changed { l.dirty = true; self.status = format!("Pilot {} updated", p.index); }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn ui_factions(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        let Some(c) = &mut l.campaign else { return };
        ui.heading("Faction standings");
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("factions").striped(true).num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
                ui.strong("Faction"); ui.strong("Standing"); ui.strong(""); ui.end_row();
                for (row, f) in l.factions.iter().enumerate() {
                    ui.label(&f.name);
                    ui.add(egui::TextEdit::singleline(&mut l.faction_edit[row]).desired_width(90.0));
                    if ui.button("Apply").clicked() {
                        if let Ok(v) = l.faction_edit[row].trim().parse::<i32>() {
                            if c.set_faction_standing(f.index, v) { l.dirty = true; self.status = format!("{} standing set to {}", f.name, v); }
                        }
                    }
                    ui.end_row();
                }
            });
        });
    }

    fn ui_coyote(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        let Some(c) = &mut l.campaign else { return };
        ui.heading("Mods with state in this save");
        ui.label("Each persistent actor is a mod or game system that stores its own data inside the save.");
        egui::Grid::new("actors").striped(true).num_columns(2).spacing([12.0, 2.0]).show(ui, |ui| {
            for a in &l.actors {
                ui.label(format!("{} bytes", a.bytes));
                ui.label(&a.class);
                ui.end_row();
            }
        });
        ui.add_space(10.0);
        if l.coyote.is_empty() {
            ui.label("Coyote's Mission Pack state not found in this save.");
            return;
        }
        egui::ScrollArea::vertical().id_source("coyote_tab").show(ui, |ui| {
        // ---- Friendly asset panel -------------------------------------------
        ui.heading("Coyote's Mission Pack: your assets");
        ui.label("Fighters, tanks and dropship turrets the pack lets you own. Changes apply immediately to the loaded save; press Save when done.");
        let mut last_group = "";
        let mut pending: Option<(usize, bool, i32)> = None; // (entry index, bool, int)
        egui::Grid::new("coyote_assets").num_columns(3).spacing([14.0, 4.0]).show(ui, |ui| {
            for (group, id, label, is_flag) in COYOTE_ASSETS {
                let Some(row) = l.coyote.iter().position(|e| &e.id == id) else { continue };
                if *group != last_group {
                    if !last_group.is_empty() { ui.end_row(); }
                    ui.strong(*group); ui.label(""); ui.label(""); ui.end_row();
                    last_group = group;
                }
                let e = &l.coyote[row];
                ui.label(format!("   {}", label));
                if *is_flag {
                    let mut v = e.bool_val;
                    if ui.checkbox(&mut v, "").changed() { pending = Some((row, v, e.int_val)); }
                    ui.label(if e.bool_val { "unlocked" } else { "locked" });
                } else {
                    ui.horizontal(|ui| {
                        if ui.small_button("-").clicked() && e.int_val > 0 { pending = Some((row, e.bool_val, e.int_val - 1)); }
                        let mut v = e.int_val;
                        if ui.add(egui::DragValue::new(&mut v).range(0..=999)).changed() { pending = Some((row, e.bool_val, v)); }
                        if ui.small_button("+").clicked() { pending = Some((row, e.bool_val, e.int_val + 1)); }
                    });
                    ui.label(if e.int_val == 0 { "none" } else { "owned" });
                }
                ui.end_row();
            }
        });
        if let Some((row, b, i)) = pending {
            let e = l.coyote[row].clone();
            if c.set_coyote_entry(e.index, &e.string_val, e.float_val, b, i) {
                l.dirty = true;
                self.status = format!("{} set to {}", e.id, if COYOTE_ASSETS.iter().any(|(_, id, _, f)| id == &e.id && *f) { b.to_string() } else { i.to_string() });
                l.coyote = c.coyote_entries();
                l.coyote_edit = l.coyote.iter().map(|e| CoyoteEdit {
                    string_val: e.string_val.clone(), float_val: e.float_val.to_string(), bool_val: e.bool_val, int_val: e.int_val.to_string(),
                }).collect();
            }
        }
        ui.add_space(12.0);
        ui.separator();

        // ---- Raw table ------------------------------------------------------
        ui.heading(format!("All Coyote entries ({})", l.coyote.len()));
        ui.label("Each entry carries a string, float, bool and int slot; most use only one. Edit the one that holds the value and press Apply.");
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut l.coyote_filter).desired_width(240.0));
            if ui.button("clear").clicked() { l.coyote_filter.clear(); }
        });
        let filter = l.coyote_filter.to_lowercase();
        egui::ScrollArea::horizontal().id_source("coyote_raw").show(ui, |ui| {
            egui::Grid::new("coyote").striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
                ui.strong("id"); ui.strong("string"); ui.strong("float"); ui.strong("bool"); ui.strong("int"); ui.strong(""); ui.end_row();
                for (row, e) in l.coyote.iter().enumerate() {
                    if !filter.is_empty() && !e.id.to_lowercase().contains(&filter) && !e.string_val.to_lowercase().contains(&filter) { continue; }
                    let ed = &mut l.coyote_edit[row];
                    ui.label(&e.id);
                    ui.add(egui::TextEdit::singleline(&mut ed.string_val).desired_width(160.0));
                    ui.add(egui::TextEdit::singleline(&mut ed.float_val).desired_width(70.0));
                    ui.checkbox(&mut ed.bool_val, "");
                    ui.add(egui::TextEdit::singleline(&mut ed.int_val).desired_width(70.0));
                    if ui.button("Apply").clicked() {
                        let f = ed.float_val.trim().parse::<f32>();
                        let i = ed.int_val.trim().parse::<i32>();
                        match (f, i) {
                            (Ok(f), Ok(i)) => {
                                if c.set_coyote_entry(e.index, &ed.string_val, f, ed.bool_val, i) {
                                    l.dirty = true;
                                    self.status = format!("Coyote entry '{}' updated", e.id);
                                } else {
                                    self.status = format!("Coyote entry '{}' unchanged", e.id);
                                }
                            }
                            _ => self.status = "float and int fields must be numbers".into(),
                        }
                    }
                    ui.end_row();
                }
            });
        });
        }); // outer scroll
    }

    fn ui_diagnose(&mut self, ui: &mut egui::Ui) {
        let Some(l) = &mut self.loaded else { return };
        ui.heading("Diagnose / Repair");
        ui.label(format!("{}  ({} bytes)", l.path.display(), l.original.len()));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Attempt repair").clicked() {
                let (bytes, outcome) = repair(&l.original);
                if outcome.changed() {
                    let rep = diagnose(&bytes);
                    l.campaign = if rep.fatal() { None } else { Campaign::open(&bytes).ok() };
                    l.report = rep;
                    l.repaired_bytes = Some(bytes);
                    l.dirty = true;
                    l.refresh();
                    self.status = format!("Repair applied {} fix(es). Review, then Save As to a new file and test it in game.", outcome.steps.len());
                } else {
                    self.status = "No automatic repair applies to this file.".into();
                }
                l.repair_outcome = Some(outcome);
            }
            ui.label("Repairs never overwrite the original until you press Save; a .bak is always written first.");
        });
        ui.add_space(8.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Some(o) = &l.repair_outcome {
                ui.strong("Repair log");
                for s in &o.steps { ui.label(format!("  fixed {:?} at {}: {}", s.kind, s.path, s.action)); }
                for u in &o.unrepairable { ui.colored_label(egui::Color32::from_rgb(220, 60, 60), format!("  {}", u)); }
                ui.add_space(8.0);
            }
            ui.strong("Findings");
            for f in &l.report.findings {
                let col = match f.severity {
                    Severity::Fatal => egui::Color32::from_rgb(220, 60, 60),
                    Severity::Suspect => egui::Color32::from_rgb(230, 170, 40),
                    Severity::Info => egui::Color32::from_rgb(70, 180, 90),
                };
                ui.colored_label(col, format!("{:?} {:?}", f.severity, f.kind));
                ui.label(format!("    {}  {}", f.path, f.detail));
            }
            ui.add_space(8.0);
            ui.strong("Log");
            for line in &self.log { ui.label(line); }
        });
    }
}
