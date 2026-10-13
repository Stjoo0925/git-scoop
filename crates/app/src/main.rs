#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use git_scoop_core::{
    index::{self, Analysis, Progress},
    store::StoredLocation,
    trace::{self, SourceEvidence, TraceEntry},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::Duration,
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1240.0, 840.0])
            .with_min_inner_size([900.0, 650.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Git Scoop",
        options,
        Box::new(|context| Ok(Box::new(GitScoop::new(context)))),
    )
}

enum AnalysisEvent {
    Progress(u64, Progress),
    Finished(u64, Result<Box<Analysis>, String>),
}
struct QueryView {
    entries: Vec<TraceEntry>,
    selected: usize,
    after: SourceEvidence,
    before: Option<SourceEvidence>,
    diff: Result<Vec<trace::DiffLine>, String>,
}
struct QueryReply {
    generation: u64,
    result: Result<QueryView, String>,
}

struct GitScoop {
    path: String,
    revision: String,
    search: String,
    loaded: Option<Analysis>,
    error: Option<String>,
    generation: u64,
    receiver: Option<Receiver<AnalysisEvent>>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: Option<(PathBuf, String)>,
    cancel: Arc<AtomicBool>,
    progress: Option<Progress>,
    filtered: Vec<(usize, usize)>,
    selected: Option<(usize, usize)>,
    query_generation: u64,
    query_receiver: Option<Receiver<QueryReply>>,
    query_cancel: Arc<AtomicBool>,
    query: Option<QueryView>,
    query_error: Option<String>,
}

impl GitScoop {
    fn new(context: &eframe::CreationContext<'_>) -> Self {
        // Use the installed Windows font as a fallback for repository data.
        // Application labels stay English; source and paths retain their text.
        if let Some(windows) = std::env::var_os("WINDIR")
            && let Ok(data) = std::fs::read(PathBuf::from(windows).join("Fonts/malgun.ttf"))
        {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "system-unicode".into(),
                Arc::new(egui::FontData::from_owned(data)),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("system-unicode".into());
            }
            context.egui_ctx.set_fonts(fonts);
        }
        let mut arguments = std::env::args().skip(1);
        let path = arguments.next().unwrap_or_default();
        let revision = arguments.next().unwrap_or_else(|| "HEAD".into());
        let mut app = Self {
            path,
            revision,
            search: String::new(),
            loaded: None,
            error: None,
            generation: 0,
            receiver: None,
            worker: None,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            filtered: Vec::new(),
            selected: None,
            query_generation: 0,
            query_receiver: None,
            query_cancel: Arc::new(AtomicBool::new(false)),
            query: None,
            query_error: None,
        };
        if !app.path.is_empty() {
            app.start(&context.egui_ctx);
        }
        app
    }

    fn start(&mut self, context: &egui::Context) {
        self.cancel.store(true, Ordering::Relaxed);
        self.query_cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.query_generation += 1;
        self.cancel = Arc::new(AtomicBool::new(false));
        self.loaded = None;
        self.error = None;
        self.progress = None;
        self.selected = None;
        self.filtered.clear();
        self.query = None;
        self.query_error = None;
        self.query_receiver = None;
        self.receiver = None;
        let path = PathBuf::from(self.path.trim());
        let revision = self.revision.trim().to_owned();
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            self.pending = Some((path, revision));
            context.request_repaint();
            return;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.launch(path, revision, context);
    }

    fn launch(&mut self, path: PathBuf, revision: String, context: &egui::Context) {
        self.pending = None;
        let (sender, receiver) = mpsc::sync_channel(4);
        self.receiver = Some(receiver);
        let cancel = self.cancel.clone();
        let context = context.clone();
        let generation = self.generation;
        self.worker = Some(std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Box<Analysis>> {
                let local = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
                    anyhow::anyhow!("The local application data directory is unavailable.")
                })?;
                let cache = index::repository_cache(&path, &PathBuf::from(local))?;
                Ok(Box::new(index::analyze(
                    &path,
                    &revision,
                    &cache,
                    &cancel,
                    |progress| {
                        let _ = sender.try_send(AnalysisEvent::Progress(generation, progress));
                        context.request_repaint();
                    },
                )?))
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = sender.send(AnalysisEvent::Finished(generation, result));
            context.request_repaint();
        }));
    }

    fn poll(&mut self, context: &egui::Context) {
        if self
            .worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            if let Some((path, revision)) = self.pending.take() {
                self.launch(path, revision, context);
            }
        }
        while let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(AnalysisEvent::Progress(generation, progress))
                    if generation == self.generation =>
                {
                    self.progress = Some(progress)
                }
                Ok(AnalysisEvent::Finished(generation, result))
                    if generation == self.generation =>
                {
                    self.receiver = None;
                    match result {
                        Ok(loaded) => {
                            self.loaded = Some(*loaded);
                            self.refresh_matches();
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Ok(_) => {}
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.receiver = None;
                    self.error = Some("The analysis worker stopped unexpectedly.".into());
                }
            }
        }
        if let Some(receiver) = &self.query_receiver {
            match receiver.try_recv() {
                Ok(reply) => {
                    self.query_receiver = None;
                    if reply.generation == self.query_generation {
                        match reply.result {
                            Ok(query) => self.query = Some(query),
                            Err(error) => self.query_error = Some(error),
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.query_receiver = None;
                    self.query_error = Some("The source query stopped unexpectedly.".into());
                }
            }
        }
    }

    fn refresh_matches(&mut self) {
        self.filtered.clear();
        let search = self.search.to_lowercase();
        if let Some(loaded) = &self.loaded {
            for (file_index, file) in loaded.snapshot.files.iter().enumerate() {
                for (symbol_index, symbol) in file.parsed.symbols.iter().enumerate() {
                    if symbol.name.to_lowercase().contains(&search) {
                        self.filtered.push((file_index, symbol_index));
                    }
                }
            }
        }
    }

    fn request_query(
        &mut self,
        selected: (usize, usize),
        row: Option<usize>,
        context: &egui::Context,
    ) {
        let Some(loaded) = &self.loaded else {
            return;
        };
        let file = &loaded.snapshot.files[selected.0];
        let Some(commit) = &loaded.snapshot.commit_id else {
            return;
        };
        let location = StoredLocation {
            commit_id: commit.clone(),
            path: file.path.clone(),
            blob_id: file.blob_id.clone(),
            symbol_index: selected.1,
        };
        let existing = row.and_then(|row| {
            self.query
                .as_ref()
                .map(|query| (query.entries.clone(), row))
        });
        self.query_cancel.store(true, Ordering::Relaxed);
        self.query_cancel = Arc::new(AtomicBool::new(false));
        self.query_generation += 1;
        self.query_error = None;
        if existing.is_none() {
            self.query = None;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        self.query_receiver = Some(receiver);
        let repository = loaded.repository.clone();
        let cache = loaded.cache_path.clone();
        let cancel = self.query_cancel.clone();
        let generation = self.query_generation;
        let context = context.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<QueryView> {
                let (entries, selected) = match existing {
                    Some(existing) => existing,
                    None => (trace::trace(&repository, &cache, &location, &cancel)?, 0),
                };
                let entry = entries.get(selected).ok_or_else(|| {
                    anyhow::anyhow!("No source evidence is available for this observation.")
                })?;
                let after = trace::source(&repository, &cache, &entry.location, &cancel)?;
                let before = entries
                    .get(selected + 1)
                    .map(|entry| trace::source(&repository, &cache, &entry.location, &cancel))
                    .transpose()?;
                let diff = match &before {
                    Some(before) => trace::line_diff(&before.lines, &after.lines)
                        .map_err(|error| error.to_string()),
                    None => Ok(Vec::new()),
                };
                Ok(QueryView {
                    entries,
                    selected,
                    after,
                    before,
                    diff,
                })
            })()
            .map_err(|error| format!("{error:#}"));
            let _ = sender.send(QueryReply { generation, result });
            context.request_repaint();
        });
    }
}

impl Drop for GitScoop {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.query_cancel.store(true, Ordering::Relaxed);
    }
}

impl eframe::App for GitScoop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll(ui.ctx());
        ui.heading("Git Scoop");
        ui.label("Code Genealogy Explorer · Rust · Local and offline");
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            ui.label("Repository");
            let path = ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(480.0));
            ui.label("Ref / commit");
            let revision =
                ui.add(egui::TextEdit::singleline(&mut self.revision).desired_width(220.0));
            if ui
                .add_enabled(
                    !self.path.trim().is_empty() && !self.revision.trim().is_empty(),
                    egui::Button::new("Analyze / Resume"),
                )
                .clicked()
                || (enter && (path.lost_focus() || revision.lost_focus()))
            {
                self.start(ui.ctx());
            }
            if (self.receiver.is_some() || self.pending.is_some()) && ui.button("Cancel").clicked()
            {
                self.cancel.store(true, Ordering::Relaxed);
                self.pending = None;
            }
        });
        ui.small("First-parent history only. Other merge parents are recorded but unanalyzed. Macros and conditional compilation are not evaluated.");
        if self.receiver.is_some() || self.pending.is_some() {
            ui.spinner();
            ui.ctx().request_repaint_after(Duration::from_millis(30));
        }
        if let Some(progress) = &self.progress {
            ui.add(
                egui::ProgressBar::new(progress.completed as f32 / progress.total.max(1) as f32)
                    .text(format!(
                        "{} / {} commits · resumed {}",
                        progress.completed, progress.total, progress.resumed
                    )),
            );
        }
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        let Some(loaded) = &self.loaded else {
            ui.label("Choose a local Git repository to explore its committed Rust code.");
            return;
        };
        let Some(commit) = &loaded.snapshot.commit_id else {
            ui.label("This repository has no commits.");
            return;
        };
        ui.label(format!("Fixed commit {commit}"));
        ui.label(format!(
            "{} first-parent commits · {} Rust files · {} symbols · {} commits with unknown scope",
            loaded.history.commits.len(),
            loaded.snapshot.files.len(),
            loaded.snapshot.symbol_count(),
            loaded.unknown_commits
        ));
        if loaded.history.boundary != git_scoop_core::git::HistoryBoundary::Root {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Incomplete ancestry: {:?}. Creation cannot be confirmed.",
                    loaded.history.boundary
                ),
            );
        }
        for skipped in &loaded.snapshot.skipped {
            ui.colored_label(
                egui::Color32::YELLOW,
                format!(
                    "Skipped {}: {}",
                    String::from_utf8_lossy(&skipped.path),
                    skipped.reason
                ),
            );
        }
        let mut refresh = false;
        ui.horizontal(|ui| {
            ui.label("Find symbols");
            refresh = ui.text_edit_singleline(&mut self.search).changed();
            ui.label(format!("{} matches", self.filtered.len()));
        });
        if refresh {
            self.refresh_matches();
        }
        ui.separator();
        let mut select = None;
        let mut timeline_row = None;
        ui.columns(2, |columns| {
            columns[0].heading("Symbols");
            if let Some(loaded) = &self.loaded {
                egui::ScrollArea::vertical()
                    .id_salt("symbols")
                    .max_height(210.0)
                    .show_rows(&mut columns[0], 22.0, self.filtered.len(), |ui, rows| {
                        for row in rows {
                            let position = self.filtered[row];
                            let file = &loaded.snapshot.files[position.0];
                            let symbol = &file.parsed.symbols[position.1];
                            let label = format!(
                                "{} {} · {}:{} · {}",
                                symbol.kind.label(),
                                symbol.name,
                                file.display_path(),
                                symbol.line,
                                symbol.context.join("::")
                            );
                            if ui
                                .add(
                                    egui::Button::selectable(
                                        self.selected == Some(position),
                                        label,
                                    )
                                    .truncate(),
                                )
                                .clicked()
                            {
                                select = Some(position);
                            }
                        }
                    });
            }
            columns[0].separator();
            columns[0].heading("Lineage timeline");
            if self.query_receiver.is_some() {
                columns[0].spinner();
                columns[0]
                    .ctx()
                    .request_repaint_after(Duration::from_millis(30));
            }
            if let Some(error) = &self.query_error {
                columns[0].colored_label(egui::Color32::LIGHT_RED, error);
            }
            if let Some(query) = &self.query {
                egui::ScrollArea::vertical()
                    .id_salt("timeline")
                    .max_height(280.0)
                    .show_rows(&mut columns[0], 22.0, query.entries.len(), |ui, rows| {
                        for row in rows {
                            let entry = &query.entries[row];
                            let label = format!(
                                "{} · {} · {}",
                                entry.name,
                                entry.events.join(" + "),
                                &entry.location.commit_id[..entry.location.commit_id.len().min(12)]
                            );
                            if ui
                                .add(
                                    egui::Button::selectable(query.selected == row, label)
                                        .truncate(),
                                )
                                .clicked()
                            {
                                timeline_row = Some(row);
                            }
                        }
                    });
            } else {
                columns[0].label("Select a symbol to inspect its history.");
            }
            columns[1].heading("Committed source evidence");
            if let Some(query) = &self.query {
                let entry = &query.entries[query.selected];
                columns[1].label(format!("{} · {}", entry.kind, entry.context.join("::")));
                columns[1].label(entry.message.lines().next().unwrap_or(""));
                columns[1].label(format!(
                    "Events: {}",
                    if entry.events.is_empty() {
                        "Preserved".into()
                    } else {
                        entry.events.join(" + ")
                    }
                ));
                source_panel(&mut columns[1], "After / selected", &query.after, "after");
                if let Some(before) = &query.before {
                    source_panel(&mut columns[1], "Earlier observation", before, "before");
                }
                if query.before.is_some() {
                    columns[1].collapsing("Source diff", |ui| match &query.diff {
                        Ok(lines) => {
                            egui::ScrollArea::both()
                                .id_salt("diff")
                                .max_height(200.0)
                                .show_rows(ui, 18.0, lines.len(), |ui, rows| {
                                    for row in rows {
                                        let line = &lines[row];
                                        let (prefix, color) = match line.kind {
                                            trace::DiffKind::Preserved => {
                                                (" ", egui::Color32::LIGHT_GRAY)
                                            }
                                            trace::DiffKind::Removed => {
                                                ("-", egui::Color32::LIGHT_RED)
                                            }
                                            trace::DiffKind::Added => {
                                                ("+", egui::Color32::LIGHT_GREEN)
                                            }
                                        };
                                        ui.label(
                                            egui::RichText::new(format!("{prefix} {}", line.text))
                                                .monospace()
                                                .color(color),
                                        );
                                    }
                                });
                        }
                        Err(error) => {
                            ui.colored_label(egui::Color32::YELLOW, error);
                        }
                    });
                }
                columns[1].collapsing("Decision evidence", |ui| {
                    for (index, parent) in entry.parents.iter().enumerate() {
                        ui.label(format!(
                            "Parent {}: {}{}",
                            index + 1,
                            parent,
                            if index == 0 {
                                " · compared"
                            } else {
                                " · unanalyzed"
                            }
                        ));
                    }
                    ui.label(&entry.evidence);
                    ui.label("Scores describe matching rules; they are not probabilities.");
                    for candidate in &entry.candidates {
                        ui.label(format!(
                            "Candidate: {} · {}",
                            String::from_utf8_lossy(&candidate.before.path),
                            candidate.evidence
                        ));
                    }
                });
            } else {
                columns[1]
                    .label("Source comes from immutable Git blobs. Your worktree is not modified.");
            }
        });
        if let Some(selected) = select {
            self.selected = Some(selected);
            self.request_query(selected, None, ui.ctx());
        } else if let (Some(selected), Some(row)) = (self.selected, timeline_row) {
            self.request_query(selected, Some(row), ui.ctx());
        }
    }
}

fn source_panel(ui: &mut egui::Ui, label: &str, source: &SourceEvidence, salt: &str) {
    ui.label(format!(
        "{label}: {} · {} · {}",
        source.name,
        String::from_utf8_lossy(&source.location.path),
        source.location.commit_id
    ));
    if source.has_error {
        ui.colored_label(
            egui::Color32::YELLOW,
            "This source has parse errors. Lineage is incomplete.",
        );
    }
    egui::ScrollArea::both()
        .id_salt(salt)
        .max_height(180.0)
        .show_rows(ui, 18.0, source.lines.len(), |ui, rows| {
            for row in rows {
                ui.monospace(format!(
                    "{:>5}  {}",
                    source.first_line + row,
                    source.lines[row]
                ));
            }
        });
}
