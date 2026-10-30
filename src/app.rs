use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};

use crate::scanner::{self, human, pretty, DiskInfo, Progress, ScanResult};

const ROW_H: f32 = 24.0;
const FULL_DISK_ACCESS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";

/// Well-known space hogs, relative to the home folder.
const KNOWN_HOGS: &[(&str, &str, &str)] = &[
    (".Trash", "Trash", "Nothing is freed until the Trash is emptied"),
    (".npm", "npm cache", "Safe: npm cache clean --force"),
    ("Library/Caches", "App caches", "Mostly safe; apps rebuild them"),
    ("Library/Developer/Xcode/DerivedData", "Xcode build data", "Safe; rebuilt on next build"),
    ("Library/Developer/Xcode/iOS DeviceSupport", "iOS device symbols", "Safe; re-downloaded when a device connects"),
    ("Library/Developer/CoreSimulator", "iOS simulators", "Delete unused simulators in Xcode"),
    ("Library/Containers/com.docker.docker", "Docker disk image", "Run: docker system prune"),
    ("Library/pnpm", "pnpm store", "Safe: pnpm store prune"),
    ("Library/Caches/Yarn", "Yarn cache", "Safe: yarn cache clean"),
    (".gradle/caches", "Gradle caches", "Safe; re-downloaded"),
    (".cache", "Tool caches", "Usually safe"),
    (".lmstudio/models", "LM Studio models", "Delete models you no longer use"),
    (".ollama/models", "Ollama models", "Run: ollama rm <model>"),
    ("Library/Android/sdk/system-images", "Android emulator images", "Remove unused images in SDK Manager"),
    (".nvm/versions/node", "Node versions (nvm)", "Run: nvm uninstall <version>"),
    (".rustup/toolchains", "Rust toolchains", "Run: rustup toolchain remove <name>"),
    ("Downloads", "Downloads", "Review old installers and archives"),
    ("Movies", "Movies", "Review large videos"),
];

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Suggestions,
    Files,
    Folders,
    Types,
}

enum Action {
    Reveal(PathBuf),
    AskTrash(Vec<PathBuf>),
    Browse(PathBuf),
    FilterFiles(String),
}

struct Suggestion {
    path: PathBuf,
    size: u64,
    label: &'static str,
    hint: &'static str,
}

pub struct App {
    root_input: String,
    progress: Option<Arc<Progress>>,
    rx: Option<mpsc::Receiver<ScanResult>>,
    scan_started: Instant,
    result: Option<ScanResult>,
    disk: Option<DiskInfo>,
    tab: Tab,
    filter: String,
    selected: HashSet<PathBuf>,
    browse: PathBuf,
    browse_files: Option<(PathBuf, Vec<(PathBuf, u64)>)>,
    confirm: Option<Vec<PathBuf>>,
    status: Option<(String, bool)>,
    suggestions: Vec<Suggestion>,
    node_modules: Vec<(PathBuf, u64)>,
    autostart: bool,
}

impl App {
    /// `initial_root`: scan this folder as soon as the window opens.
    pub fn new(initial_root: Option<String>) -> Self {
        Self {
            autostart: initial_root.is_some(),
            root_input: initial_root.unwrap_or_else(|| "/".into()),
            progress: None,
            rx: None,
            scan_started: Instant::now(),
            result: None,
            disk: scanner::disk_info(&scanner::default_root()),
            tab: match std::env::var("SPACE_HOG_TAB").as_deref() {
                Ok("folders") => Tab::Folders,
                Ok("ideas") => Tab::Suggestions,
                Ok("types") => Tab::Types,
                _ => Tab::Files,
            },
            filter: String::new(),
            selected: HashSet::new(),
            browse: PathBuf::new(),
            browse_files: None,
            confirm: None,
            status: None,
            suggestions: Vec::new(),
            node_modules: Vec::new(),
        }
    }

    fn scan_root(&self) -> PathBuf {
        let input = self.root_input.trim();
        if input == "/" || input.is_empty() {
            scanner::default_root()
        } else if let Some(rest) = input.strip_prefix('~') {
            scanner::home_dir().join(rest.trim_start_matches('/'))
        } else {
            PathBuf::from(input)
        }
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let root = self.scan_root();
        if !root.is_dir() {
            self.status = Some((format!("Not a folder: {}", root.display()), true));
            return;
        }
        let progress = Arc::new(Progress::default());
        let (tx, rx) = mpsc::channel();
        let worker_progress = progress.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = scanner::scan(root, worker_progress);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.progress = Some(progress);
        self.rx = Some(rx);
        self.scan_started = Instant::now();
        self.result = None;
        self.selected.clear();
        self.browse_files = None;
        self.status = None;
    }

    fn poll_scan(&mut self) {
        let Some(rx) = &self.rx else { return };
        if let Ok(result) = rx.try_recv() {
            self.rx = None;
            self.progress = None;
            self.browse = result.root.clone();
            self.disk = scanner::disk_info(&result.root);
            self.result = Some(result);
            self.refresh_suggestions();
        }
    }

    fn refresh_suggestions(&mut self) {
        let Some(res) = &self.result else { return };
        let home = scanner::home_dir();
        self.suggestions = KNOWN_HOGS
            .iter()
            .filter_map(|(rel, label, hint)| {
                let path = res.resolve(&home.join(rel));
                let size = res.dirs.get(&path)?.size;
                (size > 0).then_some(Suggestion { path, size, label, hint })
            })
            .collect();
        self.suggestions.sort_by(|a, b| b.size.cmp(&a.size));

        // Outermost project node_modules only; `lib/node_modules` holds global installs.
        self.node_modules = res
            .dirs
            .iter()
            .filter(|(p, _)| {
                p.file_name().is_some_and(|n| n == "node_modules")
                    && !p.parent().is_some_and(|parent| {
                        parent.ends_with("lib")
                            || parent.components().any(|c| c.as_os_str() == "node_modules")
                    })
            })
            .map(|(p, s)| (p.clone(), s.size))
            .collect();
        self.node_modules.sort_by(|a, b| b.1.cmp(&a.1));
    }

    fn trash(&mut self, paths: Vec<PathBuf>) {
        let Some(res) = &mut self.result else { return };
        #[allow(unused_mut)]
        let mut ctx = trash::TrashContext::default();
        #[cfg(target_os = "macos")]
        {
            use trash::macos::{DeleteMethod, TrashContextExtMacos};
            // Avoids the Finder automation permission prompt.
            ctx.set_delete_method(DeleteMethod::NsFileManager);
        }
        let mut moved = 0;
        let mut freed = 0;
        let mut errors = Vec::new();
        for path in paths {
            let size = res.size_of(&path);
            match ctx.delete(&path) {
                Ok(()) => {
                    res.forget(&path, size);
                    self.selected.remove(&path);
                    moved += 1;
                    freed += size;
                }
                Err(e) => errors.push(format!("{}: {e}", pretty(&path))),
            }
        }
        self.browse_files = None;
        self.disk = scanner::disk_info(&res.root);
        self.status = Some(if errors.is_empty() {
            (
                format!(
                    "Moved {moved} item(s), {}, to the Trash. Empty the Trash to free the space.",
                    human(freed)
                ),
                false,
            )
        } else {
            (format!("Moved {moved} item(s). Failed: {}", errors.join("; ")), true)
        });
        self.refresh_suggestions();
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let scanning = self.progress.is_some();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.heading(RichText::new("Space Hog").strong());
            ui.add_space(12.0);
            ui.label("Scan");
            ui.add_enabled(
                !scanning,
                egui::TextEdit::singleline(&mut self.root_input)
                    .desired_width(320.0)
                    .hint_text("/ for whole disk, ~ for home"),
            );
            if ui.add_enabled(!scanning, egui::Button::new("Whole disk")).clicked() {
                self.root_input = "/".into();
            }
            if ui.add_enabled(!scanning, egui::Button::new("Home")).clicked() {
                self.root_input = "~".into();
            }
            if scanning {
                if ui.button("Stop").clicked() {
                    if let Some(p) = &self.progress {
                        p.cancel.store(true, Relaxed);
                    }
                }
            } else if ui
                .add(egui::Button::new(RichText::new("Scan").strong()).fill(accent()))
                .clicked()
            {
                let ctx = ui.ctx().clone();
                self.start_scan(&ctx);
            }
        });

        if let Some(disk) = &self.disk {
            let frac = disk.used as f32 / disk.total.max(1) as f32;
            let fill = if frac > 0.9 { danger() } else { accent() };
            ui.add(
                egui::ProgressBar::new(frac)
                    .fill(fill)
                    .desired_height(18.0)
                    .text(format!(
                        "{} used of {}  ·  {} free",
                        human(disk.used),
                        human(disk.total),
                        human(disk.free)
                    )),
            );
        }

        if let Some(p) = &self.progress {
            ui.horizontal(|ui| {
                ui.spinner();
                let current = p.current.lock().unwrap().clone();
                ui.label(format!(
                    "{} files · {} · {:.0}s",
                    p.files.load(Relaxed),
                    human(p.bytes.load(Relaxed)),
                    self.scan_started.elapsed().as_secs_f32()
                ));
                ui.add(egui::Label::new(RichText::new(pretty(&current)).weak()).truncate());
            });
        } else if let Some(res) = &self.result {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{}{} in {} files under {} ({:.1}s).",
                    if res.cancelled { "Stopped early. Found " } else { "Found " },
                    human(res.total),
                    res.files,
                    pretty(&res.root),
                    res.elapsed.as_secs_f32()
                ));
                if let Some(disk) = &self.disk {
                    if res.root == scanner::default_root() && disk.used > res.total {
                        ui.label(RichText::new(format!(
                            "The other {} is the macOS system volume, swap, snapshots, and folders the scan could not read.",
                            human(disk.used - res.total)
                        ))
                        .weak());
                    }
                }
                if res.errors > 0 {
                    ui.label(
                        RichText::new(format!("{} items could not be read.", res.errors))
                            .color(warn()),
                    );
                    if cfg!(target_os = "macos") && ui.link("Grant Full Disk Access").clicked() {
                        open(FULL_DISK_ACCESS_URL);
                    }
                }
            });
        } else {
            ui.label(
                RichText::new("Press Scan to find what is filling your disk. Whole-disk scans take a minute or two.")
                    .weak(),
            );
        }

        if let Some((msg, is_err)) = &self.status {
            ui.horizontal(|ui| {
                ui.label(RichText::new(msg).color(if *is_err { danger() } else { ok() }));
                if !is_err && ui.small_button("Open Trash").clicked() {
                    open(&scanner::home_dir().join(".Trash").display().to_string());
                }
            });
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Files, "Largest files");
            ui.selectable_value(&mut self.tab, Tab::Folders, "Folders");
            ui.selectable_value(&mut self.tab, Tab::Suggestions, "Cleanup ideas");
            ui.selectable_value(&mut self.tab, Tab::Types, "File types");
        });
        ui.add_space(2.0);
    }

    fn files_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(res) = &self.result else { return };
        let selected = &mut self.selected;
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .desired_width(260.0)
                    .hint_text("part of path, e.g. .mp4 or Downloads"),
            );
            let total: u64 = selected.iter().map(|p| res.size_of(p)).sum();
            if ui
                .add_enabled(
                    !selected.is_empty(),
                    egui::Button::new(format!(
                        "Move {} selected to Trash ({})",
                        selected.len(),
                        human(total)
                    )),
                )
                .clicked()
            {
                actions.push(Action::AskTrash(selected.iter().cloned().collect()));
            }
            if !selected.is_empty() && ui.button("Clear selection").clicked() {
                selected.clear();
            }
        });

        let needle = self.filter.to_lowercase();
        let rows: Vec<(&scanner::FileEntry, String)> = res
            .top_files
            .iter()
            .map(|f| (f, pretty(&f.path)))
            .filter(|(_, shown)| needle.is_empty() || shown.to_lowercase().contains(&needle))
            .collect();
        let max = rows.first().map(|(f, _)| f.size).unwrap_or(1).max(1);

        TableBuilder::new(ui)
            .id_salt("files")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(22.0))
            .column(Column::exact(80.0))
            .column(Column::exact(90.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(130.0))
            .header(20.0, |mut h| {
                h.col(|_| {});
                h.col(|ui| {
                    ui.strong("Size");
                });
                h.col(|_| {});
                h.col(|ui| {
                    ui.strong(format!("Path ({} of top {})", rows.len(), scanner::TOP_FILES));
                });
                h.col(|_| {});
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let (f, shown) = &rows[row.index()];
                    row.col(|ui| {
                        let mut on = selected.contains(&f.path);
                        let protected = scanner::is_protected(&f.path, &res.root);
                        if ui.add_enabled(!protected, egui::Checkbox::without_text(&mut on)).changed() {
                            if on {
                                selected.insert(f.path.clone());
                            } else {
                                selected.remove(&f.path);
                            }
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(human(f.size)).color(size_color(f.size)).monospace());
                    });
                    row.col(|ui| size_bar(ui, f.size, max));
                    row.col(|ui| {
                        ui.add(egui::Label::new(shown.as_str()).truncate()).on_hover_text(shown);
                    });
                    row.col(|ui| row_actions(ui, &f.path, &res.root, actions));
                });
            });
    }

    fn folders_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(res) = &self.result else { return };
        if self.browse_files.as_ref().is_none_or(|(p, _)| *p != self.browse) {
            self.browse_files = Some((self.browse.clone(), files_in(&self.browse)));
        }
        let here = res.dirs.get(&self.browse).copied().unwrap_or_default();

        ui.horizontal(|ui| {
            let can_go_up = self.browse != res.root;
            if ui.add_enabled(can_go_up, egui::Button::new("⬆ Up")).clicked() {
                if let Some(parent) = self.browse.parent() {
                    actions.push(Action::Browse(parent.to_path_buf()));
                }
            }
            if ui.button("Top").clicked() {
                actions.push(Action::Browse(res.root.clone()));
            }
            ui.strong(pretty(&self.browse));
            ui.label(format!("· {} · {} files", human(here.size), here.files));
        });

        // (path, size, file count or None for files)
        let mut rows: Vec<(PathBuf, u64, Option<u64>)> = res
            .children
            .get(&self.browse)
            .into_iter()
            .flatten()
            .filter_map(|d| res.dirs.get(d).map(|s| (d.clone(), s.size, Some(s.files))))
            .collect();
        if let Some((_, files)) = &self.browse_files {
            rows.extend(files.iter().map(|(p, s)| (p.clone(), *s, None)));
        }
        rows.sort_by(|a, b| b.1.cmp(&a.1));
        let max = here.size.max(1);

        TableBuilder::new(ui)
            .id_salt("folders")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(80.0))
            .column(Column::exact(90.0))
            .column(Column::exact(48.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(90.0))
            .column(Column::exact(130.0))
            .header(20.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Size");
                });
                h.col(|_| {});
                h.col(|ui| {
                    ui.strong("%");
                });
                h.col(|ui| {
                    ui.strong("Name");
                });
                h.col(|ui| {
                    ui.strong("Files");
                });
                h.col(|_| {});
            })
            .body(|body| {
                body.rows(ROW_H, rows.len(), |mut row| {
                    let (path, size, files) = &rows[row.index()];
                    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
                    row.col(|ui| {
                        ui.label(RichText::new(human(*size)).color(size_color(*size)).monospace());
                    });
                    row.col(|ui| size_bar(ui, *size, max));
                    row.col(|ui| {
                        ui.label(RichText::new(format!("{:.0}%", *size as f64 * 100.0 / max as f64)).weak());
                    });
                    row.col(|ui| {
                        if files.is_some() {
                            if ui.link(format!("📁 {name}")).clicked() {
                                actions.push(Action::Browse(path.clone()));
                            }
                        } else {
                            ui.add(egui::Label::new(format!("📄 {name}")).truncate());
                        }
                    });
                    row.col(|ui| {
                        if let Some(n) = files {
                            ui.label(RichText::new(n.to_string()).weak());
                        }
                    });
                    row.col(|ui| row_actions(ui, path, &res.root, actions));
                });
            });
    }

    fn suggestions_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(res) = &self.result else { return };
        ui.label(
            RichText::new("Common places where space piles up. Caches regenerate when needed; anything else, check before deleting.")
                .weak(),
        );
        let suggestions = &self.suggestions;
        TableBuilder::new(ui)
            .id_salt("suggestions")
            .striped(true)
            .max_scroll_height(330.0)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(80.0))
            .column(Column::exact(180.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(130.0))
            .header(20.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Size");
                });
                h.col(|ui| {
                    ui.strong("What");
                });
                h.col(|ui| {
                    ui.strong("Advice");
                });
                h.col(|_| {});
            })
            .body(|body| {
                body.rows(ROW_H, suggestions.len(), |mut row| {
                    let s = &suggestions[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new(human(s.size)).color(size_color(s.size)).monospace());
                    });
                    row.col(|ui| {
                        ui.label(s.label).on_hover_text(pretty(&s.path));
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(s.hint).truncate());
                    });
                    row.col(|ui| {
                        if ui.small_button("Reveal").clicked() {
                            actions.push(Action::Reveal(s.path.clone()));
                        }
                        if ui.small_button("Browse").clicked() {
                            actions.push(Action::Browse(s.path.clone()));
                        }
                    });
                });
            });

        ui.add_space(10.0);
        let nm_total: u64 = self.node_modules.iter().map(|(_, s)| s).sum();
        ui.horizontal(|ui| {
            ui.strong(format!(
                "node_modules: {} in {} projects",
                human(nm_total),
                self.node_modules.len()
            ));
            ui.label(RichText::new("Safe to delete in projects you are not working on; npm install brings them back.").weak());
        });
        let nm = &self.node_modules;
        TableBuilder::new(ui)
            .id_salt("node_modules")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(80.0))
            .column(Column::remainder().clip(true))
            .column(Column::exact(130.0))
            .body(|body| {
                body.rows(ROW_H, nm.len(), |mut row| {
                    let (path, size) = &nm[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new(human(*size)).color(size_color(*size)).monospace());
                    });
                    row.col(|ui| {
                        let shown = pretty(path);
                        ui.add(egui::Label::new(&shown).truncate()).on_hover_text(&shown);
                    });
                    row.col(|ui| row_actions(ui, path, &res.root, actions));
                });
            });
    }

    fn types_tab(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let Some(res) = &self.result else { return };
        ui.label(RichText::new("Click a type to list its largest files.").weak());
        let exts = &res.extensions;
        let max = exts.first().map(|e| e.1).unwrap_or(1).max(1);
        TableBuilder::new(ui)
            .id_salt("types")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(80.0))
            .column(Column::exact(90.0))
            .column(Column::exact(160.0))
            .column(Column::remainder())
            .header(20.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Size");
                });
                h.col(|_| {});
                h.col(|ui| {
                    ui.strong("Type");
                });
                h.col(|ui| {
                    ui.strong("Files");
                });
            })
            .body(|body| {
                body.rows(ROW_H, exts.len().min(500), |mut row| {
                    let (ext, bytes, count) = &exts[row.index()];
                    row.col(|ui| {
                        ui.label(RichText::new(human(*bytes)).color(size_color(*bytes)).monospace());
                    });
                    row.col(|ui| size_bar(ui, *bytes, max));
                    row.col(|ui| {
                        let label = if ext == "(none)" { ext.clone() } else { format!(".{ext}") };
                        if ext != "(none)" && ui.link(&label).clicked() {
                            actions.push(Action::FilterFiles(format!(".{ext}")));
                        } else if ext == "(none)" {
                            ui.label(label);
                        }
                    });
                    row.col(|ui| {
                        ui.label(RichText::new(count.to_string()).weak());
                    });
                });
            });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) -> Option<bool> {
        let paths = self.confirm.as_ref()?;
        let res = self.result.as_ref()?;
        let total: u64 = paths.iter().map(|p| res.size_of(p)).sum();
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::unique("confirm-trash")).show(ctx, |ui| {
            ui.set_max_width(520.0);
            ui.heading(format!("Move {} item(s) to the Trash?", paths.len()));
            ui.label(format!("Total: {}", human(total)));
            ui.add_space(6.0);
            for p in paths.iter().take(8) {
                ui.add(egui::Label::new(RichText::new(pretty(p)).monospace()).truncate());
            }
            if paths.len() > 8 {
                ui.label(format!("…and {} more", paths.len() - 8));
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new("The space is freed only after you empty the Trash. Until then you can drag items back out of the Trash.")
                    .color(warn()),
            );
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                if ui.add(egui::Button::new("Move to Trash").fill(danger())).clicked() {
                    decision = Some(true);
                }
            });
        });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        decision
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if std::mem::take(&mut self.autostart) {
            let ctx = ui.ctx().clone();
            self.start_scan(&ctx);
        }
        self.poll_scan();
        if self.progress.is_some() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui));

        let mut actions = Vec::new();
        egui::CentralPanel::default().show(ui, |ui| {
            if self.result.is_none() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new(if self.progress.is_some() {
                        "Scanning… results appear when the scan finishes."
                    } else {
                        "No scan yet."
                    })
                    .weak());
                });
                return;
            }
            match self.tab {
                Tab::Files => self.files_tab(ui, &mut actions),
                Tab::Folders => self.folders_tab(ui, &mut actions),
                Tab::Suggestions => self.suggestions_tab(ui, &mut actions),
                Tab::Types => self.types_tab(ui, &mut actions),
            }
        });

        let ctx = ui.ctx().clone();
        match self.confirm_dialog(&ctx) {
            Some(true) => {
                let paths = self.confirm.take().unwrap_or_default();
                self.trash(paths);
            }
            Some(false) => self.confirm = None,
            None => {}
        }

        for action in actions {
            match action {
                Action::Reveal(p) => reveal(&p),
                Action::Browse(p) => {
                    self.browse = p;
                    self.tab = Tab::Folders;
                }
                Action::FilterFiles(f) => {
                    self.filter = f;
                    self.tab = Tab::Files;
                }
                Action::AskTrash(paths) => {
                    let root = self.result.as_ref().map(|r| r.root.clone()).unwrap_or_default();
                    let paths: Vec<PathBuf> =
                        paths.into_iter().filter(|p| !scanner::is_protected(p, &root)).collect();
                    if !paths.is_empty() {
                        self.confirm = Some(paths);
                    }
                }
            }
        }
    }
}

fn row_actions(ui: &mut egui::Ui, path: &Path, root: &Path, actions: &mut Vec<Action>) {
    if ui.small_button("Reveal").on_hover_text("Show in Finder").clicked() {
        actions.push(Action::Reveal(path.to_path_buf()));
    }
    let protected = scanner::is_protected(path, root);
    let trash = ui
        .add_enabled(!protected, egui::Button::new("Trash").small())
        .on_disabled_hover_text("System or protected location");
    if trash.clicked() {
        actions.push(Action::AskTrash(vec![path.to_path_buf()]));
    }
}

fn size_bar(ui: &mut egui::Ui, size: u64, max: u64) {
    let frac = (size as f64 / max as f64).clamp(0.0, 1.0) as f32;
    ui.add(
        egui::ProgressBar::new(frac)
            .desired_width(80.0)
            .desired_height(8.0)
            .fill(size_color(size)),
    );
}

fn size_color(size: u64) -> Color32 {
    match size {
        s if s >= 10_000_000_000 => danger(),
        s if s >= 1_000_000_000 => warn(),
        s if s >= 100_000_000 => accent(),
        _ => Color32::GRAY,
    }
}

fn accent() -> Color32 {
    Color32::from_rgb(64, 132, 214)
}
fn warn() -> Color32 {
    Color32::from_rgb(230, 150, 40)
}
fn danger() -> Color32 {
    Color32::from_rgb(214, 72, 64)
}
fn ok() -> Color32 {
    Color32::from_rgb(70, 170, 90)
}

/// Files (not folders) directly inside `dir`, with on-disk size.
fn files_in(dir: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
        .map(|e| {
            let path = e.path();
            let size = scanner::size_on_disk(&path);
            (path, size)
        })
        .collect()
}

fn open(target: &str) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
    #[cfg(not(target_os = "macos"))]
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

fn reveal(path: &Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(target_os = "macos"))]
    if let Some(parent) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(parent).spawn();
    }
}
