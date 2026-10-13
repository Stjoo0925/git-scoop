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
        Box::new(|context| {
            install_unicode_font(&context.egui_ctx);
            let mut arguments = std::env::args().skip(1);
            let path = arguments.next().unwrap_or_default();
            let revision = arguments.next().unwrap_or_else(|| "HEAD".into());
            let cache_root = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
            Ok(Box::new(GitScoop::new(
                &context.egui_ctx,
                path,
                revision,
                cache_root,
            )))
        }),
    )
}

// Stable widget IDs keep keyboard focus identifiable across frames.
const PATH_INPUT: &str = "repository-path";
const REVISION_INPUT: &str = "revision";
const SEARCH_INPUT: &str = "symbol-search";

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
    /// Local application data root; caches live outside analyzed repositories.
    cache_root: Option<PathBuf>,
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

/// Use the installed Windows font as a fallback for repository data.
/// Application labels stay English; source and paths retain their text.
fn install_unicode_font(context: &egui::Context) {
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
        context.set_fonts(fonts);
    }
}

impl GitScoop {
    fn new(
        context: &egui::Context,
        path: String,
        revision: String,
        cache_root: Option<PathBuf>,
    ) -> Self {
        let mut app = Self {
            cache_root,
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
            app.start(context);
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
        let cache_root = self.cache_root.clone();
        self.worker = Some(std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Box<Analysis>> {
                let local = cache_root.ok_or_else(|| {
                    anyhow::anyhow!("The local application data directory is unavailable.")
                })?;
                let cache = index::repository_cache(&path, &local)?;
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
        self.show(ui);
    }
}

impl GitScoop {
    fn show(&mut self, ui: &mut egui::Ui) {
        self.poll(ui.ctx());
        ui.heading("Git Scoop");
        ui.label("Code Genealogy Explorer · Rust · Local and offline");
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            ui.label("Repository");
            let path = ui.add(
                egui::TextEdit::singleline(&mut self.path)
                    .id(egui::Id::new(PATH_INPUT))
                    .desired_width(480.0),
            );
            ui.label("Ref / commit");
            let revision = ui.add(
                egui::TextEdit::singleline(&mut self.revision)
                    .id(egui::Id::new(REVISION_INPUT))
                    .desired_width(220.0),
            );
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
            refresh = ui
                .add(egui::TextEdit::singleline(&mut self.search).id(egui::Id::new(SEARCH_INPUT)))
                .changed();
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use std::time::Instant;

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Run one frame at the default window size, including tessellation, which
    /// the native integration also performs on the UI thread.
    fn frame(context: &egui::Context, app: &mut GitScoop, events: Vec<egui::Event>) -> Duration {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1240.0, 840.0),
            )),
            events,
            ..Default::default()
        };
        let started = Instant::now();
        let mut output = context.run_ui(input, |ui| app.show(ui));
        let shapes = std::mem::take(&mut output.shapes);
        let _ = context.tessellate(shapes, output.pixels_per_point);
        let elapsed = started.elapsed();
        // Headless frames have no GPU to upload font atlas changes to.
        output.drop_without_applying_deltas();
        elapsed
    }

    fn focused(context: &egui::Context, id: &str) -> bool {
        context.memory(|memory| memory.focused()) == Some(egui::Id::new(id))
    }

    /// Press Tab until the widget has focus, failing after a bounded search.
    fn tab_to(context: &egui::Context, app: &mut GitScoop, id: &str) -> anyhow::Result<()> {
        for _ in 0..12 {
            if focused(context, id) {
                return Ok(());
            }
            frame(context, app, vec![key(egui::Key::Tab)]);
        }
        anyhow::ensure!(focused(context, id), "Keyboard focus never reached {id}");
        Ok(())
    }

    fn wait_for(
        context: &egui::Context,
        app: &mut GitScoop,
        mut done: impl FnMut(&GitScoop) -> bool,
    ) -> anyhow::Result<()> {
        let started = Instant::now();
        while !done(app) {
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(120),
                "The UI did not reach the expected state."
            );
            frame(context, app, Vec::new());
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    #[test]
    fn keyboard_alone_analyzes_searches_and_opens_a_timeline() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        git_scoop_fixtures::generate_suite(&temporary.path().join("suite"))?;
        let repository = temporary.path().join("suite").join("types");
        let context = egui::Context::default();
        let mut app = GitScoop::new(
            &context,
            String::new(),
            "HEAD".into(),
            Some(temporary.path().join("cache")),
        );
        frame(&context, &mut app, Vec::new());
        tab_to(&context, &mut app, PATH_INPUT)?;
        frame(
            &context,
            &mut app,
            vec![egui::Event::Text(repository.display().to_string())],
        );
        tab_to(&context, &mut app, REVISION_INPUT)?;
        frame(&context, &mut app, vec![key(egui::Key::Enter)]);
        wait_for(&context, &mut app, |app| {
            app.loaded.is_some() || app.error.is_some()
        })?;
        anyhow::ensure!(app.error.is_none(), "Analysis failed: {:?}", app.error);

        tab_to(&context, &mut app, SEARCH_INPUT)?;
        let name = {
            let loaded = app.loaded.as_ref().context("Loaded analysis")?;
            loaded
                .snapshot
                .files
                .iter()
                .flat_map(|file| &file.parsed.symbols)
                .next()
                .context("Analyzed symbol")?
                .name
                .clone()
        };
        frame(&context, &mut app, vec![egui::Event::Text(name.clone())]);
        anyhow::ensure!(
            app.search == name && !app.filtered.is_empty(),
            "Typed search did not filter symbols"
        );
        // The first symbol row follows the search field in focus order.
        frame(&context, &mut app, vec![key(egui::Key::Tab)]);
        frame(&context, &mut app, vec![key(egui::Key::Enter)]);
        wait_for(&context, &mut app, |app| {
            app.query.is_some() || app.query_error.is_some()
        })?;
        anyhow::ensure!(
            app.query_error.is_none(),
            "Query failed: {:?}",
            app.query_error
        );
        anyhow::ensure!(
            app.selected == Some(app.filtered[0]),
            "Keyboard activation selected a different symbol"
        );
        Ok(())
    }

    fn p95(samples: &mut [f64]) -> f64 {
        samples.sort_by(f64::total_cmp);
        samples[(samples.len() * 95).div_ceil(100) - 1]
    }

    /// CGE-011 UI responsiveness on the 1,000-commit benchmark. Each frame
    /// carries input and is measured from input handling through
    /// tessellation; GPU presentation is outside this boundary. Run with
    /// `cargo test --release -p git-scoop -- --ignored --nocapture`.
    #[test]
    #[ignore = "generates the 1,000-commit benchmark and runs a full analysis"]
    fn ui_input_stays_responsive_during_benchmark_analysis() -> anyhow::Result<()> {
        let temporary = tempfile::tempdir()?;
        let repository = temporary.path().join("benchmark");
        git_scoop_fixtures::generate_benchmark(&repository)?;
        let context = egui::Context::default();
        let mut app = GitScoop::new(
            &context,
            repository.display().to_string(),
            "HEAD".into(),
            Some(temporary.path().join("cache")),
        );
        let mut during = Vec::new();
        let mut step = 0u32;
        let started = Instant::now();
        while app.loaded.is_none() && app.error.is_none() {
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(600),
                "Analysis did not finish."
            );
            step += 1;
            let position = egui::pos2((step * 37 % 1200) as f32, (step * 53 % 800) as f32);
            let events = vec![egui::Event::PointerMoved(position), key(egui::Key::Tab)];
            during.push(frame(&context, &mut app, events).as_secs_f64() * 1000.0);
            // The app requests a repaint every 30 ms while analysis runs.
            std::thread::sleep(Duration::from_millis(30));
        }
        anyhow::ensure!(app.error.is_none(), "Analysis failed: {:?}", app.error);
        let analysis_seconds = started.elapsed().as_secs_f64();

        // Search input after analysis filters all 100,000 declarations.
        let mut focus_steps = 0;
        while !focused(&context, SEARCH_INPUT) && focus_steps < 12 {
            frame(&context, &mut app, vec![key(egui::Key::Tab)]);
            focus_steps += 1;
        }
        anyhow::ensure!(focused(&context, SEARCH_INPUT), "Search focus failed");
        let mut search = Vec::new();
        for text in ["f", "_", "0", "1", "2"] {
            let events = vec![egui::Event::Text(text.into())];
            search.push(frame(&context, &mut app, events).as_secs_f64() * 1000.0);
        }
        let during_p95 = p95(&mut during);
        let search_max = search.iter().copied().fold(0.0, f64::max);
        println!(
            "analysis_seconds={analysis_seconds:.1} frames_during_analysis={} input_p95_ms={during_p95:.2} input_max_ms={:.2} search_max_ms={search_max:.2}",
            during.len(),
            during[during.len() - 1]
        );
        anyhow::ensure!(during.len() >= 100, "Too few samples during analysis");
        anyhow::ensure!(during_p95 <= 100.0, "UI input p95 exceeded 100 ms");
        Ok(())
    }
}
