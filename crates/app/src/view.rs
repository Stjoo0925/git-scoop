//! Screens and panels. State changes go through `GitScoop` methods so the
//! analysis and query workers keep their existing ownership rules.

use crate::{
    DetailTab, GitScoop, KindFilter, PATH_INPUT, QueryView, REVISION_INPUT, SEARCH_INPUT,
    present::{self, Tone},
    theme::{self, ACCENT, Palette},
};
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Key, Layout, Modifiers, RichText, Sense, Stroke,
};
use git_scoop_core::{
    SymbolKind,
    git::HistoryBoundary,
    trace::{DiffKind, SourceEvidence, TraceEntry},
};
use std::time::Duration;

const SYMBOL_ROW_HEIGHT: f32 = 46.0;
const CODE_ROW_HEIGHT: f32 = 19.0;

fn kind_badge(kind: &SymbolKind) -> (&'static str, Tone) {
    match kind {
        SymbolKind::Function => ("fn", Tone::Location),
        SymbolKind::Method => ("method", Tone::Identity),
        SymbolKind::TraitMethod => ("trait fn", Tone::Identity),
        SymbolKind::Struct => ("struct", Tone::Origin),
        SymbolKind::Enum => ("enum", Tone::Origin),
        SymbolKind::Trait => ("trait", Tone::Change),
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Hints for common analysis failures; the original message is always shown.
fn error_hint(message: &str) -> &'static str {
    if message.contains("Could not open a Git repository")
        || message.contains("not a git repository")
        || message.contains("appear to be a git repository")
    {
        "Choose the folder that contains the repository's .git directory."
    } else if message.contains("local branch, tag, HEAD, or full commit OID")
        || message.contains("reference")
    {
        "Check the branch, tag, or commit name. Remote branches are not read; use a local branch."
    } else if message.contains("byte limit") || message.contains("capacity") {
        "The analysis cache for this repository is full. Analysis stopped at a safe point."
    } else if message.contains("cancelled") {
        "Analysis was cancelled. Analyze again to resume from the last completed commit."
    } else {
        "Analysis stopped before finishing. Completed commits are kept and resume next time."
    }
}

impl GitScoop {
    pub(crate) fn show(&mut self, ui: &mut egui::Ui) {
        let context = ui.ctx().clone();
        self.poll(&context);
        self.handle_dropped_folder(&context);
        self.handle_shortcuts(&context);
        if self.analyzing() || self.query_receiver.is_some() {
            context.request_repaint_after(Duration::from_millis(30));
        }
        if self.loaded.is_some() {
            self.top_bar(ui);
            self.status_bar(ui);
            self.explorer(ui);
            egui::CentralPanel::default().show(ui, |ui| self.symbol_view(ui));
        } else {
            egui::CentralPanel::default().show(ui, |ui| self.start_screen(ui));
        }
        drop_overlay(&context);
    }

    fn handle_dropped_folder(&mut self, context: &egui::Context) {
        let dropped = context.input(|input| {
            input
                .raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        });
        if let Some(path) = dropped {
            // A file dropped from inside a repository opens its folder.
            let folder = if path.is_file() {
                path.parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or(path)
            } else {
                path
            };
            self.path = folder.display().to_string();
            self.start(context);
        }
    }

    fn handle_shortcuts(&mut self, context: &egui::Context) {
        if self.analyzing()
            && context.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            self.cancel_analysis();
        }
        if self.loaded.is_none() {
            return;
        }
        if context.input_mut(|input| input.consume_key(Modifiers::COMMAND, Key::F)) {
            context.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH_INPUT)));
        }
        // Arrow keys browse symbols unless another text field has focus.
        let focus = context.memory(|memory| memory.focused());
        if focus.is_some() && focus != Some(egui::Id::new(SEARCH_INPUT)) {
            return;
        }
        let step = context.input_mut(|input| {
            if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                1
            } else if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                -1
            } else {
                0
            }
        });
        if step != 0 && self.move_selection(step, context).is_some() {
            self.reveal_selected = true;
        }
    }

    // ---------------------------------------------------------------- start

    fn start_screen(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let width = ui.available_width().clamp(320.0, 620.0);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.08).clamp(16.0, 72.0));
                ui.label(RichText::new("Git Scoop").size(32.0).strong().color(palette.strong()));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "See where every Rust function and type came from: when it was created, renamed, moved, and changed.",
                    )
                    .size(15.0)
                    .color(palette.muted()),
                );
                ui.add_space(22.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        theme::card(ui).show(ui, |ui| {
                            ui.set_width(width - 30.0);
                            self.open_form(ui);
                        });
                        if !self.recent.is_empty() {
                            ui.add_space(14.0);
                            theme::card(ui).show(ui, |ui| {
                                ui.set_width(width - 30.0);
                                self.recent_list(ui);
                            });
                        }
                        ui.add_space(14.0);
                        how_it_works(ui);
                        ui.add_space(14.0);
                        ui.label(
                            RichText::new(
                                "Works offline and reads committed history only. Your files, branches, and working tree are never changed.",
                            )
                            .small()
                            .color(palette.muted()),
                        );
                    },
                );
            });
        });
    }

    fn open_form(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let busy = self.analyzing();
        ui.label(RichText::new("Repository folder").strong());
        let path = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut self.path)
                .id(egui::Id::new(PATH_INPUT))
                .hint_text("C:\\Projects\\my-crate")
                .margin(egui::Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(6.0);
        ui.label(RichText::new("Branch, tag, or commit").strong());
        let mut start = false;
        ui.horizontal(|ui| {
            let revision = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.revision)
                    .id(egui::Id::new(REVISION_INPUT))
                    .hint_text("HEAD")
                    .margin(egui::Margin::symmetric(8, 6))
                    .desired_width(200.0),
            );
            let enter = ui.input(|input| input.key_pressed(Key::Enter));
            start |= enter && (path.lost_focus() || revision.lost_focus());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if busy {
                    if ui.button("Cancel").on_hover_text("Esc").clicked() {
                        self.cancel_analysis();
                    }
                } else {
                    let ready = !self.path.trim().is_empty() && !self.revision.trim().is_empty();
                    let button = egui::Button::new(
                        RichText::new("Analyze history")
                            .color(Color32::WHITE)
                            .strong(),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(150.0, 32.0));
                    start |= ui.add_enabled(ready, button).clicked();
                }
            });
        });
        if start && !busy && !self.path.trim().is_empty() {
            self.start(ui.ctx());
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new("Tip: drop a repository folder anywhere on this window.")
                .small()
                .color(palette.muted()),
        );
        if busy {
            ui.add_space(10.0);
            self.progress_block(ui);
        }
        if let Some(error) = self.error.clone() {
            ui.add_space(10.0);
            error_box(
                ui,
                "Could not analyze this repository",
                &error,
                error_hint(&error),
            );
        }
    }

    fn progress_block(&self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        match &self.progress {
            Some(progress) => {
                let fraction = progress.completed as f32 / progress.total.max(1) as f32;
                let mut text = format!(
                    "Commit {} of {}",
                    present::count(progress.completed),
                    present::count(progress.total)
                );
                if let Some(remaining) = self.remaining_seconds(progress) {
                    text.push_str(&format!(" · {}", present::remaining(remaining)));
                }
                ui.add(egui::ProgressBar::new(fraction).fill(ACCENT).text(text));
                if progress.resumed > 0 {
                    ui.label(
                        RichText::new(format!(
                            "Resumed: {} commits were already analyzed.",
                            progress.resumed
                        ))
                        .small()
                        .color(palette.muted()),
                    );
                }
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Reading history…").color(palette.muted()));
                });
            }
        }
    }

    /// Estimate from this run's own pace; resumed commits cost nothing.
    fn remaining_seconds(&self, progress: &git_scoop_core::index::Progress) -> Option<u64> {
        let elapsed = self.started_at?.elapsed().as_secs_f64();
        let done = progress.completed.checked_sub(progress.resumed)?;
        if done < 5 || elapsed < 2.0 {
            return None;
        }
        let left = progress.total.saturating_sub(progress.completed);
        Some((elapsed / done as f64 * left as f64).ceil() as u64)
    }

    fn recent_list(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        ui.label(RichText::new("Recent").strong());
        ui.add_space(4.0);
        let mut open = None;
        let mut forget = None;
        for item in &self.recent {
            ui.horizontal(|ui| {
                // Actions take their space first so long paths truncate instead.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Remove").small()).frame(false))
                        .on_hover_text("Remove from this list")
                        .clicked()
                    {
                        forget = Some(item.path.clone());
                    }
                    theme::badge(ui, &item.revision, palette.muted());
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let label = RichText::new(item.name()).strong().color(palette.strong());
                        let response = ui
                            .add(egui::Button::new(label).frame(false))
                            .on_hover_text(item.path.display().to_string());
                        if response.clicked() {
                            open = Some(item.clone());
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new(item.path.display().to_string())
                                    .small()
                                    .color(palette.muted()),
                            )
                            .truncate(),
                        );
                    });
                });
            });
        }
        if let Some(path) = forget {
            self.forget_repository(&path);
        }
        if let Some(item) = open
            && !self.analyzing()
        {
            self.path = item.path.display().to_string();
            self.revision = item.revision;
            self.start(ui.ctx());
        }
    }

    // ------------------------------------------------------------- chrome

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        egui::Panel::top("top-bar")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Git Scoop").strong().color(ACCENT));
                    ui.separator();
                    if let Some(loaded) = &self.loaded {
                        let name = loaded
                            .repository
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.label(RichText::new(name).strong().color(palette.strong()))
                            .on_hover_text(loaded.repository.display().to_string());
                        if let Some(commit) = &loaded.snapshot.commit_id {
                            theme::badge(
                                ui,
                                &format!(
                                    "{} · {}",
                                    self.revision.trim(),
                                    present::short_id(commit)
                                ),
                                palette.muted(),
                            )
                            .on_hover_text(format!("Analysis is fixed to commit {commit}"));
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        egui::widgets::global_theme_preference_switch(ui);
                        if ui
                            .button("Close")
                            .on_hover_text("Open another repository")
                            .clicked()
                        {
                            self.close_repository();
                        }
                        if ui
                            .button("Re-analyze")
                            .on_hover_text("Analyze again; completed commits are reused")
                            .clicked()
                        {
                            self.start(ui.ctx());
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let Some(loaded) = &self.loaded else {
            return;
        };
        egui::Panel::bottom("status-bar")
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::symmetric(14, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} commits · {} Rust files · {} symbols · first-parent history",
                            present::count(loaded.history.commits.len()),
                            present::count(loaded.snapshot.files.len()),
                            present::count(loaded.snapshot.symbol_count())
                        ))
                        .small()
                        .color(palette.muted()),
                    );
                    if loaded.unknown_commits > 0 {
                        theme::badge(
                            ui,
                            &format!("{} commits not fully analyzed", loaded.unknown_commits),
                            palette.tone(Tone::Uncertain),
                        )
                        .on_hover_text(
                            "Parse errors, skipped files, or very large changes left part of these commits unexamined. Lineage never guesses across them.",
                        );
                    }
                    if loaded.history.boundary != HistoryBoundary::Root {
                        theme::badge(ui, "History is truncated", palette.tone(Tone::Uncertain))
                            .on_hover_text(
                                "The repository has a shallow or missing boundary. The oldest step is shown as first seen, not created.",
                            );
                    }
                    if !loaded.snapshot.skipped.is_empty() {
                        let list = loaded
                            .snapshot
                            .skipped
                            .iter()
                            .map(|file| {
                                format!("{}: {}", String::from_utf8_lossy(&file.path), file.reason)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        theme::badge(
                            ui,
                            &format!("{} files skipped", loaded.snapshot.skipped.len()),
                            palette.tone(Tone::Uncertain),
                        )
                        .on_hover_text(list);
                    }
                });
            });
    }

    // ----------------------------------------------------------- explorer

    fn explorer(&mut self, ui: &mut egui::Ui) {
        // Proportional limits keep room for code in small windows.
        let width = ui.available_width();
        egui::Panel::left("explorer")
            .resizable(true)
            .default_size(340.0_f32.min(width * 0.28))
            .min_size(240.0)
            .max_size((width * 0.4).max(240.0))
            .show(ui, |ui| {
                ui.add_space(8.0);
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .id(egui::Id::new(SEARCH_INPUT))
                        .hint_text("Search symbols   Ctrl+F")
                        .margin(egui::Margin::symmetric(8, 6))
                        .desired_width(f32::INFINITY),
                );
                let mut refresh = search.changed();
                ui.horizontal_wrapped(|ui| {
                    for filter in KindFilter::ALL {
                        if ui
                            .selectable_label(self.kind_filter == filter, filter.label())
                            .clicked()
                        {
                            self.kind_filter = filter;
                            refresh = true;
                        }
                    }
                });
                if refresh {
                    self.refresh_matches();
                }
                let palette = Palette::of(ui);
                ui.label(
                    RichText::new(match self.filtered.len() {
                        0 => "No symbols match".to_owned(),
                        1 => "1 symbol".to_owned(),
                        count => format!("{} symbols", present::count(count)),
                    })
                    .small()
                    .color(palette.muted()),
                );
                ui.separator();
                self.symbol_list(ui);
            });
    }

    fn symbol_list(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("symbols")
            .auto_shrink([false, false]);
        if std::mem::take(&mut self.reveal_selected)
            && let Some(row) = self
                .selected
                .and_then(|selected| self.filtered.iter().position(|item| *item == selected))
        {
            let (offset, height) = self.symbol_viewport;
            let top = row as f32 * SYMBOL_ROW_HEIGHT;
            if top < offset {
                scroll = scroll.vertical_scroll_offset(top);
            } else if top + SYMBOL_ROW_HEIGHT > offset + height {
                scroll = scroll.vertical_scroll_offset(top + SYMBOL_ROW_HEIGHT - height);
            }
        }
        let mut clicked = None;
        let Some(loaded) = &self.loaded else {
            return;
        };
        let output = scroll.show_rows(ui, SYMBOL_ROW_HEIGHT, self.filtered.len(), |ui, rows| {
            for row in rows {
                let position = self.filtered[row];
                let file = &loaded.snapshot.files[position.0];
                let symbol = &file.parsed.symbols[position.1];
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), SYMBOL_ROW_HEIGHT),
                    Sense::click(),
                );
                let selected = self.selected == Some(position);
                let painter = ui.painter_at(rect);
                let inner = rect.shrink2(egui::vec2(2.0, 2.0));
                if selected {
                    painter.rect_filled(inner, 6.0, palette.selected());
                } else if response.hovered() {
                    painter.rect_filled(inner, 6.0, palette.selected().gamma_multiply(0.5));
                }
                if response.has_focus() {
                    painter.rect_stroke(
                        inner,
                        6.0,
                        Stroke::new(1.5, ACCENT),
                        egui::StrokeKind::Inside,
                    );
                }
                let (badge, tone) = kind_badge(&symbol.kind);
                let badge_color = palette.tone(tone);
                let badge_galley = painter.layout_no_wrap(
                    badge.to_owned(),
                    FontId::proportional(11.5),
                    badge_color,
                );
                let badge_rect = egui::Rect::from_min_size(
                    inner.left_top() + egui::vec2(8.0, 7.0),
                    badge_galley.size() + egui::vec2(10.0, 2.0),
                );
                painter.rect_filled(badge_rect, 4.0, badge_color.gamma_multiply(0.16));
                painter.galley(
                    badge_rect.min + egui::vec2(5.0, 1.0),
                    badge_galley,
                    badge_color,
                );
                painter.text(
                    egui::pos2(badge_rect.right() + 8.0, badge_rect.center().y),
                    Align2::LEFT_CENTER,
                    &symbol.name,
                    FontId::proportional(14.5),
                    palette.strong(),
                );
                let context = if symbol.context.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", symbol.context.join("::"))
                };
                painter.text(
                    inner.left_top() + egui::vec2(8.0, 29.0),
                    Align2::LEFT_CENTER,
                    format!("{}:{}{}", file.display_path(), symbol.line, context),
                    FontId::proportional(12.0),
                    palette.muted(),
                );
                if response.clicked() {
                    clicked = Some(position);
                }
            }
        });
        self.symbol_viewport = (output.state.offset.y, output.inner_rect.height());
        if let Some(position) = clicked {
            self.select_symbol(position, ui.ctx());
        }
    }

    // -------------------------------------------------------- symbol view

    fn symbol_view(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let Some(selected) = self.selected else {
            empty_state(ui);
            return;
        };
        let Some(loaded) = &self.loaded else {
            return;
        };
        let file = &loaded.snapshot.files[selected.0];
        let symbol = &file.parsed.symbols[selected.1];
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (badge, tone) = kind_badge(&symbol.kind);
            theme::badge(ui, badge, palette.tone(tone));
            ui.label(
                RichText::new(&symbol.name)
                    .size(22.0)
                    .strong()
                    .color(palette.strong()),
            );
        });
        let mut location = format!("{}:{}", file.display_path(), symbol.line);
        if !symbol.context.is_empty() {
            location.push_str(&format!("  ·  in {}", symbol.context.join("::")));
        }
        ui.label(RichText::new(location).color(palette.muted()));
        ui.add_space(6.0);
        if let Some(error) = self.query_error.clone() {
            error_box(
                ui,
                "Could not load this history",
                &error,
                "Re-analyze the repository if its cache was removed or changed.",
            );
            return;
        }
        let Some(query) = &self.query else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Tracing history…").color(palette.muted()));
            });
            return;
        };
        let mut open_row = None;
        let width = ui.available_width();
        egui::Panel::left("timeline")
            .resizable(true)
            .default_size(380.0_f32.min(width * 0.45))
            .min_size(240.0)
            .max_size((width * 0.6).max(240.0))
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                right: 10,
                ..Default::default()
            }))
            .show(ui, |ui| open_row = timeline(ui, query));
        let mut tab = self.detail_tab;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                left: 6,
                ..Default::default()
            }))
            .show(ui, |ui| detail(ui, query, &mut tab));
        self.detail_tab = tab;
        if let Some(row) = open_row
            && row != query.selected
        {
            self.request_query(selected, Some(row), ui.ctx());
        }
    }
}

/// Three steps that tell a first-time user what the app does for them.
fn how_it_works(ui: &mut egui::Ui) {
    let palette = Palette::of(ui);
    const STEPS: [(&str, &str); 3] = [
        (
            "1  Analyze",
            "Reads the branch history once. Later runs resume where they stopped.",
        ),
        (
            "2  Find",
            "Search any function, method, struct, enum, or trait by name.",
        ),
        (
            "3  Trace",
            "Follow each rename, move, and change back to where it started.",
        ),
    ];
    ui.columns(3, |columns| {
        for (column, (title, body)) in columns.iter_mut().zip(STEPS) {
            theme::card(column).show(column, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(66.0);
                // Left-aligned layout keeps wrapped text from being justified.
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.label(RichText::new(title).strong().color(ACCENT));
                    ui.label(RichText::new(body).small().color(palette.muted()));
                });
            });
        }
    });
}

fn empty_state(ui: &mut egui::Ui) {
    let palette = Palette::of(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.28);
        ui.label(
            RichText::new("Pick a symbol to see its history")
                .size(20.0)
                .color(palette.strong()),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("Search on the left, or use ↑ and ↓ to move through the list.")
                .color(palette.muted()),
        );
        ui.label(
            RichText::new(
                "Each step shows what changed, who changed it, and why the link was made.",
            )
            .color(palette.muted()),
        );
    });
}

fn error_box(ui: &mut egui::Ui, title: &str, message: &str, hint: &str) {
    let palette = Palette::of(ui);
    let color = palette.tone(Tone::Gone);
    egui::Frame::new()
        .fill(color.gamma_multiply(0.10))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).strong().color(color));
            ui.label(hint);
            ui.label(RichText::new(message).small().color(palette.muted()));
        });
}

fn drop_overlay(context: &egui::Context) {
    if context.input(|input| input.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = context.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop-overlay"),
    ));
    let rect = context.content_rect();
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(170));
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        "Drop a repository folder to analyze it",
        FontId::proportional(22.0),
        Color32::WHITE,
    );
}

/// Draw the lineage steps, newest first, and return a clicked row.
fn timeline(ui: &mut egui::Ui, query: &QueryView) -> Option<usize> {
    let palette = Palette::of(ui);
    let now = now_seconds();
    let mut clicked = None;
    ui.label(RichText::new("History").strong().color(palette.strong()));
    ui.label(
        RichText::new(match query.entries.len() {
            1 => "1 step, newest first".to_owned(),
            count => format!("{count} steps, newest first"),
        })
        .small()
        .color(palette.muted()),
    );
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("timeline")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (row, entry) in query.entries.iter().enumerate() {
                if timeline_step(ui, entry, row == query.selected, now).clicked() {
                    clicked = Some(row);
                }
                ui.add_space(4.0);
            }
            if let Some(oldest) = query.entries.last()
                && !oldest.events.iter().any(|event| event == "Created")
            {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "History stops here: earlier steps are not linked with enough certainty.",
                    )
                    .small()
                    .color(palette.muted()),
                );
            }
        });
    clicked
}

fn timeline_step(
    ui: &mut egui::Ui,
    entry: &TraceEntry,
    selected: bool,
    now: i64,
) -> egui::Response {
    let palette = Palette::of(ui);
    let tone = entry
        .events
        .first()
        .map(|kind| present::event_label(kind).1)
        .unwrap_or(Tone::Quiet);
    let frame = egui::Frame::new()
        .fill(if selected {
            palette.selected()
        } else {
            palette.card()
        })
        .stroke(Stroke::new(
            1.0,
            if selected { ACCENT } else { palette.border() },
        ))
        .corner_radius(8)
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), Sense::hover());
                ui.painter()
                    .circle_filled(dot.center(), 5.0, palette.tone(tone));
                ui.horizontal_wrapped(|ui| {
                    if entry.events.is_empty() {
                        theme::badge(ui, "Unchanged", palette.tone(Tone::Quiet))
                            .on_hover_text(present::event_hint("Unchanged"));
                    }
                    for kind in &entry.events {
                        let (label, tone) = present::event_label(kind);
                        let hint = present::event_hint(kind);
                        let badge = theme::badge(ui, label, palette.tone(tone));
                        if !hint.is_empty() {
                            badge.on_hover_text(hint);
                        }
                    }
                });
            });
            let message = present::first_line(&entry.message);
            ui.label(
                RichText::new(if message.is_empty() {
                    "(no message)"
                } else {
                    message
                })
                .color(palette.strong()),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {} · {}",
                    if entry.author.is_empty() {
                        "unknown author"
                    } else {
                        &entry.author
                    },
                    present::when(entry.authored_at, now),
                    present::short_id(&entry.location.commit_id)
                ))
                .small()
                .color(palette.muted()),
            );
            // Name and file per step make renames and moves visible at a glance.
            ui.label(
                RichText::new(format!(
                    "{} in {}",
                    entry.name,
                    String::from_utf8_lossy(&entry.location.path)
                ))
                .small()
                .color(palette.muted()),
            );
        });
    ui.interact(
        frame.response.rect,
        frame.response.id.with("step"),
        Sense::click(),
    )
}

fn detail(ui: &mut egui::Ui, query: &QueryView, tab: &mut DetailTab) {
    let palette = Palette::of(ui);
    let entry = &query.entries[query.selected];
    if query.before.is_none() && *tab == DetailTab::Changes {
        *tab = DetailTab::Code;
    }
    ui.horizontal(|ui| {
        ui.selectable_value(tab, DetailTab::Code, "Code");
        ui.add_enabled_ui(query.before.is_some(), |ui| {
            ui.selectable_value(tab, DetailTab::Changes, "Changes")
                .on_disabled_hover_text("This is the oldest linked step.");
        });
        ui.selectable_value(tab, DetailTab::Evidence, "Why linked");
    });
    ui.separator();
    match tab {
        DetailTab::Code => {
            source_header(ui, &query.after);
            code_view(ui, &query.after, "code");
        }
        DetailTab::Changes => {
            if let Some(before) = &query.before {
                ui.label(
                    RichText::new(format!(
                        "From {} at {} to {} at {}",
                        before.name,
                        present::short_id(&before.location.commit_id),
                        query.after.name,
                        present::short_id(&query.after.location.commit_id)
                    ))
                    .color(palette.muted()),
                );
                match &query.diff {
                    Ok(lines) => diff_view(ui, lines),
                    Err(error) => {
                        ui.label(RichText::new(error).color(palette.tone(Tone::Uncertain)));
                    }
                }
            }
        }
        DetailTab::Evidence => evidence_view(ui, entry),
    }
}

fn source_header(ui: &mut egui::Ui, source: &SourceEvidence) {
    let palette = Palette::of(ui);
    ui.label(
        RichText::new(format!(
            "{} at commit {}",
            String::from_utf8_lossy(&source.location.path),
            present::short_id(&source.location.commit_id)
        ))
        .small()
        .color(palette.muted()),
    );
    if source.has_error {
        ui.label(
            RichText::new("This file has syntax errors, so lineage around it is incomplete.")
                .small()
                .color(palette.tone(Tone::Uncertain)),
        );
    }
}

fn code_view(ui: &mut egui::Ui, source: &SourceEvidence, salt: &str) {
    let palette = Palette::of(ui);
    let font = FontId::monospace(13.0);
    let longest = source
        .lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'm'));
    egui::Frame::new()
        .fill(palette.code_background())
        .corner_radius(6)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt(salt)
                .auto_shrink([false, false])
                .show_rows(ui, CODE_ROW_HEIGHT, source.lines.len(), |ui, rows| {
                    let width = (64.0 + longest as f32 * char_width).max(ui.available_width());
                    for row in rows {
                        let number = source.first_line + row;
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, CODE_ROW_HEIGHT),
                            Sense::hover(),
                        );
                        let painter = ui.painter();
                        if source.symbol_lines.contains(&number) {
                            painter.rect_filled(rect, 0.0, palette.highlight_line());
                        }
                        painter.text(
                            rect.left_center() + egui::vec2(40.0, 0.0),
                            Align2::RIGHT_CENTER,
                            number.to_string(),
                            font.clone(),
                            palette.muted(),
                        );
                        painter.text(
                            rect.left_center() + egui::vec2(56.0, 0.0),
                            Align2::LEFT_CENTER,
                            &source.lines[row],
                            font.clone(),
                            palette.strong(),
                        );
                    }
                });
        });
}

fn diff_view(ui: &mut egui::Ui, lines: &[git_scoop_core::trace::DiffLine]) {
    let palette = Palette::of(ui);
    let font = FontId::monospace(13.0);
    let longest = lines
        .iter()
        .map(|line| line.text.chars().count())
        .max()
        .unwrap_or(0);
    let char_width = ui.fonts_mut(|fonts| fonts.glyph_width(&font, 'm'));
    egui::Frame::new()
        .fill(palette.code_background())
        .corner_radius(6)
        .inner_margin(egui::Margin::same(6))
        .show(ui, |ui| {
            egui::ScrollArea::both()
                .id_salt("diff")
                .auto_shrink([false, false])
                .show_rows(ui, CODE_ROW_HEIGHT, lines.len(), |ui, rows| {
                    let width = (40.0 + longest as f32 * char_width).max(ui.available_width());
                    for row in rows {
                        let line = &lines[row];
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(width, CODE_ROW_HEIGHT),
                            Sense::hover(),
                        );
                        let (sign, background, color) = match line.kind {
                            DiffKind::Added => {
                                ("+", Some(palette.added()), palette.tone(Tone::Origin))
                            }
                            DiffKind::Removed => {
                                ("-", Some(palette.removed()), palette.tone(Tone::Gone))
                            }
                            DiffKind::Preserved => (" ", None, palette.muted()),
                        };
                        let painter = ui.painter();
                        if let Some(background) = background {
                            painter.rect_filled(rect, 0.0, background);
                        }
                        painter.text(
                            rect.left_center() + egui::vec2(8.0, 0.0),
                            Align2::LEFT_CENTER,
                            sign,
                            font.clone(),
                            color,
                        );
                        painter.text(
                            rect.left_center() + egui::vec2(24.0, 0.0),
                            Align2::LEFT_CENTER,
                            &line.text,
                            font.clone(),
                            palette.strong(),
                        );
                    }
                });
        });
}

fn evidence_view(ui: &mut egui::Ui, entry: &TraceEntry) {
    let palette = Palette::of(ui);
    egui::ScrollArea::vertical()
        .id_salt("evidence")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for kind in &entry.events {
                let (label, tone) = present::event_label(kind);
                ui.horizontal(|ui| {
                    theme::badge(ui, label, palette.tone(tone));
                    ui.label(RichText::new(present::event_hint(kind)).color(palette.muted()));
                });
            }
            ui.add_space(8.0);
            match present::link_evidence(&entry.evidence) {
                Some(evidence) => {
                    ui.label(RichText::new("Why this is the same declaration").strong());
                    for check in &evidence.checks {
                        let (mark, color) = if check.held {
                            ("✔", palette.tone(Tone::Origin))
                        } else {
                            ("✖", palette.muted())
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(mark).color(color));
                            ui.label(RichText::new(check.label).color(if check.held {
                                palette.strong()
                            } else {
                                palette.muted()
                            }));
                        });
                    }
                    if let Some(score) = evidence.score {
                        ui.label(
                            RichText::new(format!(
                                "Rule score {score}. Scores rank matching rules; they are not probabilities."
                            ))
                            .small()
                            .color(palette.muted()),
                        );
                    }
                }
                None => {
                    ui.label(RichText::new("What was recorded").strong());
                    for line in entry.evidence.lines() {
                        ui.label(RichText::new(line).color(palette.muted()));
                    }
                }
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Compared commits").strong());
            for (index, parent) in entry.parents.iter().enumerate() {
                let note = if index == 0 {
                    "first parent, compared"
                } else {
                    "merge parent, not analyzed"
                };
                ui.label(
                    RichText::new(format!("{}  {note}", present::short_id(parent)))
                        .color(palette.muted()),
                )
                .on_hover_text(parent);
            }
            if entry.parents.is_empty() {
                ui.label(RichText::new("Root commit, no parent").color(palette.muted()));
            }
            if !entry.candidates.is_empty() {
                ui.add_space(8.0);
                ui.label(RichText::new("Other possibilities, not linked").strong());
                for candidate in &entry.candidates {
                    ui.label(
                        RichText::new(format!(
                            "{} · {} at {}",
                            present::candidate_label(&candidate.evidence),
                            String::from_utf8_lossy(&candidate.before.path),
                            present::short_id(&candidate.before.commit_id)
                        ))
                        .color(palette.muted()),
                    );
                }
            }
        });
}
