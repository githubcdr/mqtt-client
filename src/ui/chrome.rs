//! Top status/connection bar and bottom event-log bar.

use crate::ui::{draw_status_dot, format_bytes};
use crate::{ConnectionState, MqttApp};
use eframe::egui::{self, Color32, RichText, ScrollArea};

impl MqttApp {
    pub(crate) fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar")
            .exact_size(46.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let btn = ui.button("   Profiles");
                    let tri_center = egui::pos2(btn.rect.left() + 12.0, btn.rect.center().y);
                    let tri_color = ui.style().interact(&btn).text_color();
                    let dx = if self.show_profile_sidebar { -3.5 } else { 3.5 };
                    let pts = vec![
                        egui::pos2(tri_center.x + dx, tri_center.y),
                        egui::pos2(tri_center.x - dx, tri_center.y - 4.0),
                        egui::pos2(tri_center.x - dx, tri_center.y + 4.0),
                    ];
                    ui.painter().add(egui::Shape::convex_polygon(
                        pts,
                        tri_color,
                        egui::Stroke::NONE,
                    ));
                    if btn.clicked() {
                        self.show_profile_sidebar = !self.show_profile_sidebar;
                    }

                    ui.separator();

                    let current_name = self.current_profile().name.clone();
                    egui::ComboBox::from_id_salt("profile_select")
                        .selected_text(RichText::new(current_name).strong())
                        .width(185.0)
                        .show_ui(ui, |ui| {
                            for idx in 0..self.config.profiles.len() {
                                let label = format!(
                                    "{} ({}:{})",
                                    self.config.profiles[idx].name,
                                    self.config.profiles[idx].host,
                                    self.config.profiles[idx].port
                                );
                                if ui
                                    .selectable_value(
                                        &mut self.config.selected_profile_idx,
                                        idx,
                                        label,
                                    )
                                    .changed()
                                {
                                    self.active_version = self.config.profiles[idx].version;
                                }
                            }
                        });

                    let ver_label = self.current_profile().version.short_badge();
                    ui.label(
                        RichText::new(ver_label)
                            .color(Color32::from_rgb(130, 200, 255))
                            .strong(),
                    );

                    if self.current_profile().tls.enabled {
                        ui.label(
                            RichText::new("TLS")
                                .color(Color32::from_rgb(110, 230, 150))
                                .strong(),
                        );
                    }

                    match self.conn_state {
                        ConnectionState::Disconnected => {
                            let btn = egui::Button::new(
                                RichText::new("Connect")
                                    .color(Color32::WHITE)
                                    .strong(),
                            )
                            .fill(Color32::from_rgb(38, 132, 72));
                            if ui.add(btn).clicked() {
                                self.connect_selected_profile();
                            }
                        }
                        ConnectionState::Connecting => {
                            let btn = egui::Button::new(
                                RichText::new("Cancel")
                                    .color(Color32::WHITE)
                                    .strong(),
                            )
                            .fill(Color32::from_rgb(180, 120, 30));
                            if ui.add(btn).clicked() {
                                self.disconnect();
                            }
                        }
                        ConnectionState::Connected => {
                            let btn = egui::Button::new(
                                RichText::new("Disconnect")
                                    .color(Color32::WHITE)
                                    .strong(),
                            )
                            .fill(Color32::from_rgb(185, 50, 50));
                            if ui.add(btn).clicked() {
                                self.disconnect();
                            }
                        }
                    }

                    let (status_txt, status_col) = match self.conn_state {
                        ConnectionState::Disconnected => {
                            ("Disconnected", Color32::from_rgb(150, 155, 165))
                        }
                        ConnectionState::Connecting => {
                            ("Connecting...", Color32::from_rgb(245, 190, 65))
                        }
                        ConnectionState::Connected => {
                            ("Connected", Color32::from_rgb(75, 225, 120))
                        }
                    };
                    draw_status_dot(ui, 4.5, status_col);
                    ui.label(RichText::new(status_txt).color(status_col).strong());

                    ui.separator();

                    let mpm = self.tree.messages_per_minute();
                    ui.label(format!(
                        "Topics: {}  |  Rx: {} ({})  |  {:.0} msg/min",
                        self.tree.total_unique_topics,
                        self.tree.total_messages_received,
                        format_bytes(self.tree.total_bytes_received),
                        mpm,
                    ));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("+ Sample Data")
                            .on_hover_text(
                                "Inject sample traffic across multiple root topics to preview root-topic distribution, throughput graph, and payload decoders",
                            )
                            .clicked()
                        {
                            self.inject_sample_traffic();
                        }
                    });
                });
            });
    }

    pub(crate) fn render_bottom_bar(
        &mut self,
        ui: &mut egui::Ui,
        available: egui::Rect,
    ) -> egui::Rect {
        let panel_id = if self.show_logs {
            "bottom_status_bar_open"
        } else {
            "bottom_status_bar_closed"
        };
        let mut panel = egui::Panel::bottom(panel_id);
        if self.show_logs {
            panel = panel
                .resizable(true)
                .default_size(160.0)
                .min_size(70.0)
                .max_size(480.0);
        } else {
            panel = panel.resizable(false);
        }

        // Render the bottom bar on Order::Middle so its background and top resize
        // handle sit above the workspace panels (Order::Background) and below
        // popups/tooltips (Order::Foreground).
        let bottom_layer =
            egui::LayerId::new(egui::Order::Middle, ui.scope_id().with("event_log_layer"));
        let mut layer_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(available)
                .layer_id(bottom_layer),
        );
        panel
            .show(&mut layer_ui, |ui| {
                ui.horizontal(|ui| {
                    let toggle_txt = if self.show_logs {
                        "Hide Event Log"
                    } else {
                        "Event Log"
                    };
                    if ui.small_button(toggle_txt).clicked() {
                        self.show_logs = !self.show_logs;
                    }
                    ui.separator();
                    if let Some(latest) = self.logs.front() {
                        ui.label(
                            RichText::new(latest)
                                .monospace()
                                .size(11.5)
                                .color(Color32::from_rgb(190, 198, 212)),
                        );
                    }
                });

                if self.show_logs {
                    ui.separator();
                    ScrollArea::vertical()
                        .max_height(ui.available_height())
                        .auto_shrink([false, false])
                        .id_salt("event_log_scroll")
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            for line in &self.logs {
                                let color = if line.contains("ERROR") || line.contains("rejected") {
                                    Color32::from_rgb(255, 110, 110)
                                } else if line.contains("Connected") {
                                    Color32::from_rgb(100, 230, 140)
                                } else {
                                    Color32::from_rgb(185, 192, 205)
                                };
                                ui.label(RichText::new(line).monospace().size(11.5).color(color));
                            }
                        });
                }
            })
            .response
            .rect
    }
}
