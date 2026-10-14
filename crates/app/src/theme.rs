//! Typography, spacing, and colors for both light and dark themes.

use crate::{highlight::Token, present::Tone};
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Theme, Visuals,
};

pub const ACCENT: Color32 = Color32::from_rgb(0x3B, 0x82, 0xF6);

/// Colors that follow the active theme.
#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
}

impl Palette {
    pub fn of(ui: &egui::Ui) -> Self {
        Self {
            dark: ui.visuals().dark_mode,
        }
    }

    pub fn muted(self) -> Color32 {
        if self.dark {
            Color32::from_gray(150)
        } else {
            Color32::from_gray(105)
        }
    }

    pub fn strong(self) -> Color32 {
        if self.dark {
            Color32::from_gray(235)
        } else {
            Color32::from_gray(20)
        }
    }

    pub fn card(self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x1E, 0x22, 0x2A)
        } else {
            Color32::WHITE
        }
    }

    pub fn border(self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x2E, 0x34, 0x3F)
        } else {
            Color32::from_rgb(0xDD, 0xE1, 0xE7)
        }
    }

    pub fn selected(self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x1E, 0x33, 0x55)
        } else {
            Color32::from_rgb(0xE3, 0xED, 0xFD)
        }
    }

    pub fn code_background(self) -> Color32 {
        if self.dark {
            Color32::from_rgb(0x15, 0x18, 0x1E)
        } else {
            Color32::from_rgb(0xF6, 0xF8, 0xFA)
        }
    }

    pub fn highlight_line(self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 28)
        } else {
            Color32::from_rgba_unmultiplied(0x3B, 0x82, 0xF6, 22)
        }
    }

    pub fn added(self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(0x22, 0xC5, 0x5E, 40)
        } else {
            Color32::from_rgb(0xE6, 0xF6, 0xEA)
        }
    }

    pub fn removed(self) -> Color32 {
        if self.dark {
            Color32::from_rgba_unmultiplied(0xEF, 0x44, 0x44, 40)
        } else {
            Color32::from_rgb(0xFD, 0xEB, 0xEC)
        }
    }

    /// Code token colors tuned for each theme's code background.
    pub fn syntax(self, token: Token) -> Color32 {
        let (dark, light) = match token {
            Token::Plain => return self.strong(),
            Token::Keyword => ((0xC6, 0x78, 0xDD), (0x8B, 0x2F, 0xC9)),
            Token::Type => ((0xE5, 0xC0, 0x7B), (0x98, 0x68, 0x01)),
            Token::Function => ((0x61, 0xAF, 0xEF), (0x1F, 0x5F, 0xBF)),
            Token::Macro => ((0x56, 0xB6, 0xC2), (0x0E, 0x7C, 0x86)),
            Token::String => ((0x98, 0xC3, 0x79), (0x3A, 0x7D, 0x2A)),
            Token::Number => ((0xD1, 0x9A, 0x66), (0xB0, 0x52, 0x10)),
            Token::Comment => ((0x7F, 0x84, 0x8E), (0x6A, 0x73, 0x7D)),
            Token::Lifetime => ((0xE0, 0x6C, 0x75), (0xC0, 0x25, 0x2D)),
        };
        let (r, g, b) = if self.dark { dark } else { light };
        Color32::from_rgb(r, g, b)
    }

    /// Foreground color for an event category, readable in both themes.
    pub fn tone(self, tone: Tone) -> Color32 {
        let (dark, light) = match tone {
            Tone::Origin => ((0x4A, 0xDE, 0x80), (0x15, 0x80, 0x3D)),
            Tone::Identity => ((0xC0, 0x84, 0xFC), (0x7E, 0x22, 0xCE)),
            Tone::Location => ((0x60, 0xA5, 0xFA), (0x1D, 0x4E, 0xD8)),
            Tone::Change => ((0xFB, 0xBF, 0x24), (0xB4, 0x53, 0x09)),
            Tone::Quiet => ((0x9C, 0xA3, 0xAF), (0x4B, 0x55, 0x63)),
            Tone::Uncertain => ((0xFB, 0x92, 0x3C), (0xC2, 0x41, 0x0C)),
            Tone::Gone => ((0xF8, 0x71, 0x71), (0xB9, 0x1C, 0x1C)),
        };
        let (r, g, b) = if self.dark { dark } else { light };
        Color32::from_rgb(r, g, b)
    }
}

/// Apply typography and spacing to both themes and follow the system theme.
pub fn install(context: &egui::Context) {
    context.options_mut(|options| options.theme_preference = egui::ThemePreference::System);
    for theme in [Theme::Dark, Theme::Light] {
        context.style_mut_of(theme, |style| {
            style.text_styles = [
                (
                    TextStyle::Heading,
                    FontId::new(20.0, FontFamily::Proportional),
                ),
                (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
                (
                    TextStyle::Button,
                    FontId::new(14.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Small,
                    FontId::new(12.0, FontFamily::Proportional),
                ),
                (
                    TextStyle::Monospace,
                    FontId::new(13.0, FontFamily::Monospace),
                ),
            ]
            .into();
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(12.0, 6.0);
            style.spacing.interact_size.y = 28.0;
            style.spacing.text_edit_width = 240.0;
            style.visuals = visuals(theme);
        });
    }
}

fn visuals(theme: Theme) -> Visuals {
    let dark = theme == Theme::Dark;
    let mut visuals = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    let palette = Palette { dark };
    visuals.panel_fill = if dark {
        Color32::from_rgb(0x16, 0x19, 0x1F)
    } else {
        Color32::from_rgb(0xF4, 0xF5, 0xF7)
    };
    visuals.window_fill = palette.card();
    visuals.extreme_bg_color = palette.code_background();
    // Inputs need to stand out from panels and cards in both themes.
    visuals.text_edit_bg_color = Some(if dark {
        Color32::from_rgb(0x0F, 0x11, 0x16)
    } else {
        Color32::WHITE
    });
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.hyperlink_color = ACCENT;
    let radius = CornerRadius::same(6);
    for widget in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widget.corner_radius = radius;
    }
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, palette.border());
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, palette.border());
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    visuals
}

/// A rounded card used to group related content.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let palette = Palette::of(ui);
    egui::Frame::new()
        .fill(palette.card())
        .stroke(Stroke::new(1.0, palette.border()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
}

/// A small colored label for an event or symbol kind.
pub fn badge(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(12.0).color(color))
        })
        .inner
}
