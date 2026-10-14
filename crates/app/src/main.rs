#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod highlight;
mod present;
mod recent;
mod theme;
mod view;

use eframe::egui;
use git_scoop_core::{
    SymbolKind,
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
};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Git Scoop")
            .with_inner_size([1320.0, 860.0])
            .with_min_inner_size([960.0, 640.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "Git Scoop",
        options,
        Box::new(|context| {
            install_unicode_font(&context.egui_ctx);
            theme::install(&context.egui_ctx);
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

/// Symbol groups offered as filters in the explorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindFilter {
    All,
    Functions,
    Methods,
    Types,
}

impl KindFilter {
    const ALL: [Self; 4] = [Self::All, Self::Functions, Self::Methods, Self::Types];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Functions => "Functions",
            Self::Methods => "Methods",
            Self::Types => "Types",
        }
    }

    fn accepts(self, kind: &SymbolKind) -> bool {
        match self {
            Self::All => true,
            Self::Functions => matches!(kind, SymbolKind::Function),
            Self::Methods => matches!(kind, SymbolKind::Method | SymbolKind::TraitMethod),
            Self::Types => matches!(
                kind,
                SymbolKind::Struct | SymbolKind::Enum | SymbolKind::Trait
            ),
        }
    }
}

/// Views of the selected timeline entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Code,
    Changes,
    Evidence,
}

struct GitScoop {
    /// Local application data root; caches live outside analyzed repositories.
    cache_root: Option<PathBuf>,
    path: String,
    revision: String,
    search: String,
    kind_filter: KindFilter,
    detail_tab: DetailTab,
    recent: Vec<recent::RecentRepository>,
    loaded: Option<Analysis>,
    error: Option<String>,
    generation: u64,
    receiver: Option<Receiver<AnalysisEvent>>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: Option<(PathBuf, String)>,
    cancel: Arc<AtomicBool>,
    progress: Option<Progress>,
    /// When the running analysis started, for the remaining-time estimate.
    started_at: Option<std::time::Instant>,
    filtered: Vec<(usize, usize)>,
    selected: Option<(usize, usize)>,
    /// Scroll the symbol list to the selection on the next frame.
    reveal_selected: bool,
    /// Symbol list scroll offset and visible height from the last frame.
    symbol_viewport: (f32, f32),
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

impl eframe::App for GitScoop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl GitScoop {
    fn new(
        context: &egui::Context,
        path: String,
        revision: String,
        cache_root: Option<PathBuf>,
    ) -> Self {
        let recent = cache_root.as_deref().map(recent::load).unwrap_or_default();
        let mut app = Self {
            cache_root,
            path,
            revision,
            search: String::new(),
            kind_filter: KindFilter::All,
            detail_tab: DetailTab::Code,
            recent,
            loaded: None,
            error: None,
            generation: 0,
            receiver: None,
            worker: None,
            pending: None,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            started_at: None,
            filtered: Vec::new(),
            selected: None,
            reveal_selected: false,
            symbol_viewport: (0.0, 0.0),
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

    fn analyzing(&self) -> bool {
        self.receiver.is_some() || self.pending.is_some()
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

    /// Stop any analysis and return to the start screen.
    fn close_repository(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.query_cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.query_generation += 1;
        self.pending = None;
        self.receiver = None;
        self.query_receiver = None;
        self.loaded = None;
        self.progress = None;
        self.error = None;
        self.selected = None;
        self.filtered.clear();
        self.query = None;
        self.query_error = None;
    }

    fn cancel_analysis(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.pending = None;
    }

    fn launch(&mut self, path: PathBuf, revision: String, context: &egui::Context) {
        self.pending = None;
        self.started_at = Some(std::time::Instant::now());
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
                            self.remember_repository();
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

    /// Keep successfully analyzed repositories one click away. The list is a
    /// convenience, so a failed write leaves analysis results untouched.
    fn remember_repository(&mut self) {
        let (Some(root), Some(loaded)) = (&self.cache_root, &self.loaded) else {
            return;
        };
        let _ = recent::remember(
            root,
            &mut self.recent,
            recent::RecentRepository {
                path: loaded.repository.clone(),
                revision: self.revision.trim().to_owned(),
                opened_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok()),
                saved: false,
            },
        );
    }

    fn forget_repository(&mut self, path: &std::path::Path) {
        if let Some(root) = &self.cache_root {
            let _ = recent::forget(root, &mut self.recent, path);
        }
    }

    fn save_repository(&mut self, path: &std::path::Path, saved: bool) {
        if let Some(root) = &self.cache_root {
            let _ = recent::set_saved(root, &mut self.recent, path, saved);
        }
    }

    /// Whether the open repository is on the saved list.
    fn repository_saved(&self) -> Option<bool> {
        let loaded = self.loaded.as_ref()?;
        self.recent
            .iter()
            .find(|item| item.path == loaded.repository)
            .map(|item| item.saved)
    }

    fn refresh_matches(&mut self) {
        self.filtered.clear();
        let search = self.search.trim().to_lowercase();
        if let Some(loaded) = &self.loaded {
            for (file_index, file) in loaded.snapshot.files.iter().enumerate() {
                for (symbol_index, symbol) in file.parsed.symbols.iter().enumerate() {
                    if self.kind_filter.accepts(&symbol.kind)
                        && (search.is_empty() || symbol.name.to_lowercase().contains(&search))
                    {
                        self.filtered.push((file_index, symbol_index));
                    }
                }
            }
        }
    }

    fn select_symbol(&mut self, selected: (usize, usize), context: &egui::Context) {
        self.selected = Some(selected);
        self.request_query(selected, None, context);
    }

    /// Move the symbol selection by `step` rows within the filtered list.
    fn move_selection(&mut self, step: isize, context: &egui::Context) -> Option<usize> {
        if self.filtered.is_empty() {
            return None;
        }
        let current = self
            .selected
            .and_then(|selected| self.filtered.iter().position(|item| *item == selected));
        let last = self.filtered.len() - 1;
        let next = match current {
            None => 0,
            Some(index) => index.saturating_add_signed(step).min(last),
        };
        if current != Some(next) {
            self.select_symbol(self.filtered[next], context);
        }
        Some(next)
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use std::time::{Duration, Instant};

    fn key(key: egui::Key) -> egui::Event {
        key_with(key, egui::Modifiers::NONE)
    }

    fn key_with(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
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
        theme::install(&context);
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

        frame(
            &context,
            &mut app,
            vec![key_with(egui::Key::F, egui::Modifiers::COMMAND)],
        );
        frame(&context, &mut app, Vec::new());
        anyhow::ensure!(
            focused(&context, SEARCH_INPUT),
            "Ctrl+F did not focus search"
        );
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
        // Arrow keys browse the filtered symbols while search has focus.
        frame(&context, &mut app, vec![key(egui::Key::ArrowDown)]);
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
        theme::install(&context);
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
        frame(
            &context,
            &mut app,
            vec![key_with(egui::Key::F, egui::Modifiers::COMMAND)],
        );
        frame(&context, &mut app, Vec::new());
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
