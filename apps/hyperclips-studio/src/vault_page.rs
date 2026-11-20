//! Encrypted vault page: create / unlock, list, open, extract, remove, and
//! move clips in from the library (the original is deleted after encryption).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use eframe::egui;
use hc_core::i18n::Localizer;
use hc_core::paths::AppPaths;
use hc_vault::{EntryId, Vault};

const MIN_PASSWORD: usize = 8;

pub struct VaultPage {
    dir: PathBuf,
    library_db: PathBuf,
    export_dir: PathBuf,
    vault: Option<Arc<Vault>>,
    password: String,
    confirm: String,
    new_password: String,
    message: Option<(bool, String)>,
    /// Library items waiting for the vault to be unlocked.
    pending: Vec<(i64, PathBuf)>,
    busy: Option<Receiver<Result<String, String>>>,
    confirm_remove: Option<EntryId>,
}

impl VaultPage {
    pub fn new(paths: &AppPaths, clips_root: PathBuf) -> Self {
        Self {
            dir: paths.data_dir.join("vault"),
            library_db: paths.library_db(),
            export_dir: clips_root.join("Vault"),
            vault: None,
            password: String::new(),
            confirm: String::new(),
            new_password: String::new(),
            message: None,
            pending: Vec::new(),
            busy: None,
            confirm_remove: None,
        }
    }

    /// Queue a library item to be moved into the vault.
    pub fn request_add(&mut self, id: i64, path: PathBuf) {
        self.pending.push((id, path));
    }

    fn start_pending(&mut self, i: &Localizer) {
        if self.busy.is_some() || self.pending.is_empty() {
            return;
        }
        let Some(v) = self.vault.clone() else { return };
        let jobs = std::mem::take(&mut self.pending);
        let db = self.library_db.clone();
        let done = i.tr("vault-added");
        let (tx, rx) = channel();
        let _ = std::thread::Builder::new()
            .name("vault-add".into())
            .spawn(move || {
                let mut n = 0;
                for (id, path) in jobs {
                    match v.add_file(&path, true) {
                        Ok(_) => {
                            n += 1;
                            if let Ok(lib) = hc_library::Library::open(&db) {
                                let _ = lib.forget(id);
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(format!("{}: {e}", path.display())));
                            return;
                        }
                    }
                }
                let _ = tx.send(Ok(format!("{done} ({n})")));
            });
        self.busy = Some(rx);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        ui.heading(i.tr("vault-title"));
        ui.small(i.tr("vault-hint"));
        ui.separator();
        if let Some(rx) = &self.busy {
            match rx.try_recv() {
                Ok(r) => {
                    self.message = Some(match r {
                        Ok(m) => (true, m),
                        Err(e) => (false, e),
                    });
                    self.busy = None;
                }
                Err(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(i.tr("vault-working"));
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                }
            }
        }
        if let Some((ok, m)) = &self.message {
            let c = if *ok {
                ui.visuals().hyperlink_color
            } else {
                ui.visuals().error_fg_color
            };
            ui.colored_label(c, m);
        }
        if !self.pending.is_empty() && self.vault.is_none() {
            ui.label(i.tr_args(
                "vault-pending",
                &[("count", &self.pending.len().to_string())],
            ));
        }
        match self.vault.clone() {
            None if !Vault::exists(&self.dir) => self.create_ui(ui, i),
            None => self.unlock_ui(ui, i),
            Some(v) => {
                self.start_pending(i);
                self.list_ui(ui, i, &v);
            }
        }
    }

    fn create_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        ui.label(i.tr("vault-create-intro"));
        ui.horizontal(|ui| {
            ui.label(i.tr("vault-password"));
            ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
        });
        ui.horizontal(|ui| {
            ui.label(i.tr("vault-confirm"));
            ui.add(egui::TextEdit::singleline(&mut self.confirm).password(true));
        });
        ui.colored_label(ui.visuals().warn_fg_color, i.tr("vault-no-recovery"));
        let ok = self.password.chars().count() >= MIN_PASSWORD && self.password == self.confirm;
        if ui
            .add_enabled(ok, egui::Button::new(i.tr("vault-create")))
            .clicked()
        {
            match Vault::create(&self.dir, &self.password) {
                Ok(v) => {
                    self.vault = Some(Arc::new(v));
                    self.message = None;
                }
                Err(e) => self.message = Some((false, e.to_string())),
            }
            self.password.clear();
            self.confirm.clear();
        }
    }

    fn unlock_ui(&mut self, ui: &mut egui::Ui, i: &Localizer) {
        let mut go = false;
        ui.horizontal(|ui| {
            ui.label(i.tr("vault-password"));
            let r = ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
            go = r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter));
            go |= ui.button(i.tr("vault-unlock")).clicked();
        });
        if go {
            match Vault::unlock(&self.dir, &self.password) {
                Ok(v) => {
                    self.vault = Some(Arc::new(v));
                    self.message = None;
                }
                Err(hc_vault::Error::WrongPassword) => {
                    self.message = Some((false, i.tr("vault-wrong-password")));
                }
                Err(e) => self.message = Some((false, e.to_string())),
            }
            self.password.clear();
        }
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, i: &Localizer, v: &Arc<Vault>) {
        ui.horizontal(|ui| {
            if ui.button(i.tr("vault-lock")).clicked() {
                // Dropping the last handle wipes the key and temp exports.
                self.vault = None;
            }
            ui.label(i.tr("vault-new-password"));
            ui.add(
                egui::TextEdit::singleline(&mut self.new_password)
                    .password(true)
                    .desired_width(160.0),
            );
            let ok = self.new_password.chars().count() >= MIN_PASSWORD && self.busy.is_none();
            if ui
                .add_enabled(ok, egui::Button::new(i.tr("vault-change-password")))
                .clicked()
            {
                // Re-wrapping needs exclusive access; only possible when idle.
                if let Some(mut owned) = self.vault.take().and_then(|a| Arc::try_unwrap(a).ok()) {
                    self.message = Some(match owned.change_password(&self.new_password) {
                        Ok(()) => (true, i.tr("vault-password-changed")),
                        Err(e) => (false, e.to_string()),
                    });
                    self.vault = Some(Arc::new(owned));
                }
                self.new_password.clear();
            }
        });
        if self.vault.is_none() {
            return;
        }
        ui.separator();
        let entries = v.list();
        if entries.is_empty() {
            ui.label(i.tr("vault-empty"));
            return;
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("vault-list")
                .striped(true)
                .num_columns(4)
                .show(ui, |ui| {
                    for e in entries {
                        ui.label(&e.name);
                        ui.label(hc_core::sys::format_bytes(e.size));
                        ui.label(hc_core::sys::format_local(e.added_unix as i64));
                        ui.horizontal(|ui| {
                            if ui.button(i.tr("vault-open")).clicked() {
                                match v.export_temp(e.id) {
                                    Ok(p) => {
                                        let _ = hc_library::fsops::open_default(&p);
                                    }
                                    Err(err) => self.message = Some((false, err.to_string())),
                                }
                            }
                            if ui.button(i.tr("vault-extract")).clicked() {
                                let dest = unique(&self.export_dir.join(&e.name));
                                let r = std::fs::create_dir_all(&self.export_dir)
                                    .map_err(|x| x.to_string())
                                    .and_then(|()| {
                                        v.extract(e.id, &dest).map_err(|x| x.to_string())
                                    });
                                self.message = Some(match r {
                                    Ok(()) => (true, dest.display().to_string()),
                                    Err(x) => (false, x),
                                });
                            }
                            if self.confirm_remove == Some(e.id) {
                                if ui.button(i.tr("lib-yes")).clicked() {
                                    if let Err(x) = v.remove(e.id) {
                                        self.message = Some((false, x.to_string()));
                                    }
                                    self.confirm_remove = None;
                                }
                                if ui.button(i.tr("lib-no")).clicked() {
                                    self.confirm_remove = None;
                                }
                            } else if ui.button(i.tr("lib-delete")).clicked() {
                                self.confirm_remove = Some(e.id);
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    }
}

fn unique(p: &std::path::Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| p.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or_else(|| p.to_path_buf())
}
