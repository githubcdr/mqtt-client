//! egui view layer. Each submodule implements its slice of the interface
//! as rendering methods on `MqttApp` (defined in `main.rs`).

mod charts;
mod chrome;
mod inspector;
mod publish;
mod sidebar;
mod tree_view;

use crate::protocol::MqttVersion;
use eframe::egui::{self, Color32, Ui};

impl MqttVersion {
    pub(crate) fn short_badge(&self) -> &'static str {
        match self {
            MqttVersion::V31 => "v3.1",
            MqttVersion::V311 => "v3.1.1",
            MqttVersion::V5 => "v5.0",
        }
    }
}

pub(crate) fn format_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
    }
}

/// Paint a crisp anti-aliased status dot without relying on Unicode font glyphs.
pub(crate) fn draw_status_dot(ui: &mut Ui, radius: f32, color: Color32) {
    let side = (radius * 2.0 + 4.0).max(10.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
}
