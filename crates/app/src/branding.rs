//! Shared embedded branding for the native window and application screens.

use eframe::egui::{self, TextureHandle};
use std::sync::{Arc, OnceLock};

const ICON_PNG: &[u8] = include_bytes!("../../../assets/branding/git-scoop-256.png");

pub fn icon() -> Arc<egui::IconData> {
    static ICON: OnceLock<Arc<egui::IconData>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(
            eframe::icon_data::from_png_bytes(ICON_PNG).expect("Embedded app icon is valid PNG"),
        )
    })
    .clone()
}

pub fn show(ui: &mut egui::Ui, size: f32) {
    let id = egui::Id::new("git-scoop-brand-texture");
    let cached = ui.ctx().data(|data| data.get_temp::<TextureHandle>(id));
    let texture = cached.unwrap_or_else(|| {
        let icon = icon();
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let texture = ui
            .ctx()
            .load_texture("Git Scoop", image, egui::TextureOptions::LINEAR);
        ui.ctx()
            .data_mut(|data| data.insert_temp(id, texture.clone()));
        texture
    });
    ui.add(egui::Image::new((texture.id(), egui::vec2(size, size))).alt_text("Git Scoop logo"));
}
