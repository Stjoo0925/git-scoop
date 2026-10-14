//! Screens and panels. State changes go through `GitScoop` methods so the
//! analysis and query workers keep their existing ownership rules.

use crate::{
    DetailTab, GitScoop, KindFilter, PATH_INPUT, QueryView, REVISION_INPUT, SEARCH_INPUT,
    highlight,
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
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            egui::widgets::global_theme_preference_switch(ui);
        });
        // Two columns on wide windows: what the app does, then the one thing
        // to do next. Narrow windows put that action right after the heading.
        egui::ScrollArea::vertical().show(ui, |ui| {
            let total = ui.available_width();
            let wide = total >= 1060.0;
            // Stacked layouts keep a readable line length.
            let content = if wide {
                total.min(1080.0) - 48.0
            } else {
                total.min(680.0) - 48.0
            };
            ui.add_space((ui.available_height() * 0.1).clamp(24.0, 96.0));
            ui.horizontal_top(|ui| {
                ui.add_space(((total - content) / 2.0).max(24.0));
                if wide {
                    let gap = 56.0;
                    let left = (content - gap) * 0.5;
                    let right = content - gap - left;
                    ui.allocate_ui_with_layout(
                        egui::vec2(left, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.add_space(12.0);
                            hero_heading(ui);
                            ui.add_space(28.0);
                            hero_points(ui);
                        },
                    );
                    ui.add_space(gap);
                    ui.allocate_ui_with_layout(
                        egui::vec2(right, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| self.open_column(ui, right),
                    );
                } else {
                    ui.allocate_ui_with_layout(
                        egui::vec2(content, 0.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            // The action comes before the details when stacked.
                            hero_heading(ui);
                            ui.add_space(24.0);
                            self.open_column(ui, content);
                            ui.add_space(28.0);
                            hero_points(ui);
                        },
                    );
                }
            });
            ui.add_space(32.0);
        });
    }

    fn open_column(&mut self, ui: &mut egui::Ui, width: f32) {
        theme::card(ui)
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(width - 42.0);
                self.open_form(ui);
            });
        if !self.recent.is_empty() {
            ui.add_space(20.0);
            self.recent_list(ui, width);
        }
    }

    fn open_form(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(ui);
        let busy = self.analyzing();
        ui.label(
            RichText::new("Open a repository")
                .size(18.0)
                .color(palette.strong()),
        );
        ui.add_space(12.0);
        drop_zone(ui, busy);
        ui.add_space(14.0);
        ui.label(RichText::new("Folder path").small().color(palette.muted()));
        let path = ui.add_enabled(
            !busy,
            egui::TextEdit::singleline(&mut self.path)
                .id(egui::Id::new(PATH_INPUT))
                .hint_text("C:\\Projects\\my-crate")
                .margin(egui::Margin::symmetric(10, 8))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(10.0);
        ui.label(
            RichText::new("Branch, tag, or commit")
                .small()
                .color(palette.muted()),
        );
        let mut start = false;
        ui.horizontal(|ui| {
            let revision = ui.add_enabled(
                !busy,
                egui::TextEdit::singleline(&mut self.revision)
                    .id(egui::Id::new(REVISION_INPUT))
                    .hint_text("HEAD")
                    .margin(egui::Margin::symmetric(10, 8))
                    .desired_width(160.0),
            );
            let enter = ui.input(|input| input.key_pressed(Key::Enter));
            start |= enter && (path.lost_focus() || revision.lost_focus());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if busy {
                    if ui
                        .add(egui::Button::new("Cancel").min_size(egui::vec2(96.0, 36.0)))
                        .on_hover_text("Esc")
                        .clicked()
                    {
                        self.cancel_analysis();
                    }
                } else {
                    let ready = !self.path.trim().is_empty() && !self.revision.trim().is_empty();
                    let button = egui::Button::new(
                        RichText::new("Analyze history  →")
                            .size(15.0)
                            .color(Color32::WHITE),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(172.0, 36.0));
                    start |= ui
                        .add_enabled(ready, button)
                        .on_disabled_hover_text("Enter or drop a repository folder first")
                        .clicked();
                }
            });
        });
        if start && !busy && !self.path.trim().is_empty() {
            self.start(ui.ctx());
        }
        if busy {
            ui.add_space(16.0);
            self.progress_block(ui);
        }
        if let Some(error) = self.error.clone() {
            ui.add_space(16.0);
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

    fn recent_list(&mut self, ui: &mut egui::Ui, width: f32) {
        let palette = Palette::of(ui);
        let now = now_seconds();
        ui.label(RichText::new("RECENT").size(11.5).color(palette.muted()));
        ui.add_space(6.0);
        let mut open = None;
        let mut forget = None;
        for item in &self.recent {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 54.0), Sense::click());
            let painter = ui.painter_at(rect);
            let hovered = response.hovered() || response.has_focus();
            painter.rect(
                rect.shrink(1.0),
                8.0,
                if hovered {
                    palette.selected()
                } else {
                    palette.card()
                },
                Stroke::new(1.0, if hovered { ACCENT } else { palette.border() }),
                egui::StrokeKind::Inside,
            );
            // An initial avatar makes repositories recognizable at a glance.
            let avatar = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 30.0, rect.center().y),
                egui::vec2(32.0, 32.0),
            );
            painter.rect_filled(avatar, 8.0, ACCENT.gamma_multiply(0.18));
            painter.text(
                avatar.center(),
                Align2::CENTER_CENTER,
                item.name()
                    .chars()
                    .next()
                    .unwrap_or('?')
                    .to_uppercase()
                    .to_string(),
                FontId::proportional(16.0),
                ACCENT,
            );
            let text_left = avatar.right() + 12.0;
            let actions = 120.0;
            let text_width = (rect.right() - actions - text_left).max(40.0);
            let name = painter.layout(
                item.name(),
                FontId::proportional(15.0),
                palette.strong(),
                text_width,
            );
            painter.galley(
                egui::pos2(text_left, rect.top() + 9.0),
                name,
                palette.strong(),
            );
            let mut detail = egui::text::LayoutJob::simple_singleline(
                item.path.display().to_string(),
                FontId::proportional(12.0),
                palette.muted(),
            );
            detail.wrap = egui::text::TextWrapping::truncate_at_width(text_width);
            painter.galley(
                egui::pos2(text_left, rect.top() + 31.0),
                painter.layout_job(detail),
                palette.muted(),
            );
            let opened = item
                .opened_at
                .map(|seconds| present::when(Some(seconds), now))
                .unwrap_or_default();
            painter.text(
                egui::pos2(rect.right() - 16.0, rect.top() + 18.0),
                Align2::RIGHT_CENTER,
                &item.revision,
                FontId::proportional(12.5),
                palette.strong(),
            );
            painter.text(
                egui::pos2(rect.right() - 16.0, rect.top() + 37.0),
                Align2::RIGHT_CENTER,
                opened,
                FontId::proportional(11.5),
                palette.muted(),
            );
            let response = response.on_hover_text(format!(
                "Open {} at {}\nRight-click to remove from this list",
                item.path.display(),
                item.revision
            ));
            if response.clicked() {
                open = Some(item.clone());
            }
            response.context_menu(|ui| {
                if ui.button("Remove from recent").clicked() {
                    forget = Some(item.path.clone());
                    ui.close();
                }
            });
            ui.add_space(4.0);
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
                    logo(ui, 20.0);
                    ui.label(RichText::new("Git Scoop").color(palette.strong()));
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

/// The product pitch: what the app answers.
fn hero_heading(ui: &mut egui::Ui) {
    let palette = Palette::of(ui);
    ui.horizontal(|ui| {
        logo(ui, 30.0);
        ui.label(
            RichText::new("Git Scoop")
                .size(19.0)
                .color(palette.strong()),
        );
    });
    ui.add_space(26.0);
    ui.label(
        // A deliberate break keeps the headline from ending on an orphan.
        RichText::new("Know where your\ncode came from.")
            .size(34.0)
            .color(palette.strong()),
    );
    ui.add_space(12.0);
    ui.label(
        RichText::new(
            "Pick any Rust function or type and follow it back through every rename, move, and edit, to the commit where it began.",
        )
        .size(16.0)
        .color(palette.muted()),
    );
}

/// Why the answers can be trusted.
fn hero_points(ui: &mut egui::Ui) {
    let palette = Palette::of(ui);
    const POINTS: [(&str, &str, Tone); 3] = [
        (
            "Follows renames and moves",
            "Lineage survives new names and new files, not just line edits.",
            Tone::Identity,
        ),
        (
            "Shows its reasoning",
            "Every step lists what matched. Uncertain links are never guessed.",
            Tone::Location,
        ),
        (
            "Private and read-only",
            "Runs offline on committed history and never changes your files.",
            Tone::Origin,
        ),
    ];
    for (title, body, tone) in POINTS {
        ui.horizontal_top(|ui| {
            let (dot, _) = ui.allocate_exact_size(egui::vec2(18.0, 22.0), Sense::hover());
            ui.painter().circle_filled(
                dot.center() + egui::vec2(0.0, 1.0),
                4.5,
                palette.tone(tone),
            );
            ui.vertical(|ui| {
                ui.label(RichText::new(title).size(15.0).color(palette.strong()));
                ui.label(RichText::new(body).color(palette.muted()));
            });
        });
        ui.add_space(14.0);
    }
}

/// A small mark: a history line whose steps branch off a main trunk.
fn logo(ui: &mut egui::Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, size * 0.26, ACCENT);
    let x = rect.left() + size * 0.36;
    let top = egui::pos2(x, rect.top() + size * 0.24);
    let bottom = egui::pos2(x, rect.bottom() - size * 0.24);
    let branch = egui::pos2(rect.right() - size * 0.28, rect.center().y);
    let stroke = Stroke::new(size * 0.08, Color32::WHITE);
    painter.line_segment([top, bottom], stroke);
    painter.line_segment([egui::pos2(x, rect.top() + size * 0.36), branch], stroke);
    for point in [top, bottom, branch] {
        painter.circle_filled(point, size * 0.11, Color32::WHITE);
    }
}

/// The primary way in: a dashed target that lights up while files hover.
fn drop_zone(ui: &mut egui::Ui, busy: bool) {
    let palette = Palette::of(ui);
    let hovering = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 112.0), Sense::hover());
    let painter = ui.painter();
    if hovering {
        painter.rect_filled(rect, 10.0, ACCENT.gamma_multiply(0.10));
    } else {
        painter.rect_filled(rect, 10.0, palette.code_background());
    }
    let inset = rect.shrink(1.0);
    painter.extend(egui::Shape::dashed_line(
        &[
            inset.left_top(),
            inset.right_top(),
            inset.right_bottom(),
            inset.left_bottom(),
            inset.left_top(),
        ],
        Stroke::new(
            1.5,
            if hovering {
                ACCENT
            } else {
                // Muted text color reads as a border in both themes.
                palette.muted().gamma_multiply(0.7)
            },
        ),
        6.0,
        5.0,
    ));
    // Folder glyph drawn with shapes so it needs no icon font.
    let icon = egui::Rect::from_center_size(
        rect.center() - egui::vec2(0.0, 18.0),
        egui::vec2(34.0, 26.0),
    );
    let color = if hovering { ACCENT } else { palette.muted() };
    painter.rect_filled(
        egui::Rect::from_min_size(icon.left_top(), egui::vec2(14.0, 8.0)),
        3.0,
        color,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(icon.left_top() + egui::vec2(0.0, 4.0), icon.right_bottom()),
        4.0,
        color,
    );
    let (title, detail) = if busy {
        (
            "Analysis in progress",
            "You can still browse once it finishes.",
        )
    } else if hovering {
        (
            "Release to analyze",
            "The folder's committed history will be read.",
        )
    } else {
        ("Drop a repository folder here", "or paste its path below")
    };
    painter.text(
        rect.center() + egui::vec2(0.0, 18.0),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(15.0),
        if hovering { ACCENT } else { palette.strong() },
    );
    painter.text(
        rect.center() + egui::vec2(0.0, 38.0),
        Align2::CENTER_CENTER,
        detail,
        FontId::proportional(12.5),
        palette.muted(),
    );
}

fn empty_state(ui: &mut egui::Ui) {
    let palette = Palette::of(ui);
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.24);
        logo(ui, 44.0);
        ui.add_space(16.0);
        ui.label(
            RichText::new("Pick a symbol to trace its history")
                .size(22.0)
                .color(palette.strong()),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new("You will see every step from today back to where it began.")
                .color(palette.muted()),
        );
        ui.add_space(22.0);
        // Shortcuts as key caps; a fixed width keeps the rows aligned.
        let rows: [(&[&str], &str); 3] = [
            (&["Ctrl", "F"], "Search symbols"),
            (&["↑", "↓"], "Move through the list"),
            (&["Right-click"], "Copy a step's commit ID"),
        ];
        ui.allocate_ui_with_layout(egui::vec2(320.0, 0.0), Layout::top_down(Align::Min), |ui| {
            for (keys, action) in rows {
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(130.0, 24.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            for key in keys.iter().rev() {
                                key_cap(ui, key);
                            }
                        },
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new(action).color(palette.muted()));
                });
                ui.add_space(4.0);
            }
        });
    });
}

fn key_cap(ui: &mut egui::Ui, key: &str) {
    let palette = Palette::of(ui);
    egui::Frame::new()
        .fill(palette.card())
        .stroke(Stroke::new(1.0, palette.border()))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(key).size(12.0).color(palette.strong()));
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
            let last = query.entries.len().saturating_sub(1);
            for (row, entry) in query.entries.iter().enumerate() {
                let response = ui
                    .horizontal(|ui| {
                        // The gutter holds the connecting line and step marker.
                        ui.add_space(GUTTER);
                        timeline_step(ui, entry, row == query.selected, now)
                    })
                    .inner;
                let rect = response.rect;
                let x = rect.left() - GUTTER / 2.0 - 2.0;
                let marker = egui::pos2(x, rect.top() + 24.0);
                if row < last {
                    ui.painter().line_segment(
                        [marker, egui::pos2(x, rect.bottom() + 12.0 + 24.0)],
                        Stroke::new(2.0, palette.border()),
                    );
                }
                let tone = entry
                    .events
                    .first()
                    .map(|kind| present::event_label(kind).1)
                    .unwrap_or(Tone::Quiet);
                ui.painter().circle_filled(marker, 6.0, palette.tone(tone));
                ui.painter()
                    .circle_stroke(marker, 6.0, Stroke::new(2.0, ui.visuals().panel_fill));
                if response.clicked() {
                    clicked = Some(row);
                }
                let commit = entry.location.commit_id.clone();
                response.context_menu(|ui| {
                    if ui.button("Copy commit ID").clicked() {
                        ui.ctx().copy_text(commit.clone());
                        ui.close();
                    }
                });
                ui.add_space(6.0);
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

const GUTTER: f32 = 22.0;

fn timeline_step(
    ui: &mut egui::Ui,
    entry: &TraceEntry,
    selected: bool,
    now: i64,
) -> egui::Response {
    let palette = Palette::of(ui);
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
            // The step sits in a horizontal row beside its gutter.
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
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
        });
    ui.interact(
        frame.response.rect,
        frame.response.id.with("step"),
        Sense::click(),
    )
    .on_hover_text("Click to inspect · right-click to copy the commit ID")
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

/// One source line as a colored text layout.
fn highlighted(
    text: &str,
    state: &mut highlight::State,
    palette: Palette,
    font: &FontId,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    for (range, token) in highlight::line(text, state) {
        job.append(
            &text[range],
            0.0,
            egui::TextFormat::simple(font.clone(), palette.syntax(token)),
        );
    }
    job
}

/// Tokenizer state at the start of each line, so visible rows can be colored
/// without rendering the rows above them.
fn line_states<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<highlight::State> {
    let mut state = highlight::State::default();
    lines
        .map(|line| {
            let start = state;
            highlight::line(line, &mut state);
            start
        })
        .collect()
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
    let states = line_states(source.lines.iter().map(String::as_str));
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
                        let mut state = states[row];
                        let galley = painter.layout_job(highlighted(
                            &source.lines[row],
                            &mut state,
                            palette,
                            &font,
                        ));
                        let top = rect.center().y - galley.size().y / 2.0;
                        painter.galley(
                            egui::pos2(rect.left() + 56.0, top),
                            galley,
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
    let states = line_states(lines.iter().map(|line| line.text.as_str()));
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
                        let mut state = states[row];
                        let galley =
                            painter.layout_job(highlighted(&line.text, &mut state, palette, &font));
                        let top = rect.center().y - galley.size().y / 2.0;
                        painter.galley(
                            egui::pos2(rect.left() + 24.0, top),
                            galley,
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
