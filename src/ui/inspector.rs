//! Right-hand inspector panel: selected message payload, diff view, value
//! chart, and the layout of the publish drawer and chart sections.

use crate::payload::{format_payload, PayloadFormat};
use crate::tree::compute_payload_diff;
use crate::tree::DiffTag;
use crate::ui::charts::render_sparkline_chart;
use crate::{InspectorCache, InspectorTab, MqttApp};
use eframe::egui::{self, Color32, FontFamily, FontId, RichText, ScrollArea, TextFormat, Ui};

impl MqttApp {
    pub(crate) fn render_right_panel(&mut self, ui: &mut Ui) {
        let available_h = ui.available_height();
        let root_h = if self.show_root_topics_section {
            168.0_f32
        } else {
            30.0_f32
        };
        let throughput_h = if self.show_throughput_section {
            162.0_f32
        } else {
            30.0_f32
        };
        let graphs_reserved = (root_h + throughput_h).min((available_h * 0.65).max(60.0));

        let upper_h = (available_h - graphs_reserved).max(80.0);
        let pub_h = if self.show_publish_drawer {
            170.0_f32.min((upper_h * 0.35).max(80.0))
        } else {
            0.0_f32
        };
        let inspector_h = if self.show_publish_drawer {
            (upper_h - pub_h - 32.0).max(80.0)
        } else {
            (upper_h - 28.0).max(80.0)
        };

        ui.allocate_ui(egui::vec2(ui.available_width(), inspector_h), |ui| {
            self.render_inspector_section(ui);
        });

        ui.add_space(4.0);

        let header_resp =
            egui::CollapsingHeader::new(RichText::new("Publish Message Drawer").strong())
                .id_salt("publish_drawer_collapsing")
                .default_open(true)
                .open(Some(self.show_publish_drawer))
                .show(ui, |ui| {
                    if let Some(sel) = self.selected_topic.clone() {
                        ui.horizontal(|ui| {
                            if ui.small_button("Use Selected Topic").clicked() {
                                self.publish_draft.topic = sel;
                            }
                        });
                    }
                    ui.allocate_ui(egui::vec2(ui.available_width(), pub_h), |ui| {
                        ScrollArea::vertical()
                            .id_salt("publish_drawer_scroll")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                self.render_publish_drawer(ui);
                            });
                    });
                });
        if header_resp.header_response.clicked() {
            self.show_publish_drawer = !self.show_publish_drawer;
        }

        ui.add_space(4.0);
        self.render_root_topics_collapsing(ui);
        ui.add_space(4.0);
        self.render_throughput_collapsing(ui);
    }

    fn render_inspector_section(&mut self, ui: &mut Ui) {
        let Some(topic_path) = self.selected_topic.clone() else {
            return;
        };

        let Some(node) = self.tree.find_node(&topic_path) else {
            ui.label("Selected topic not found.");
            return;
        };

        ui.horizontal(|ui| {
            let max_topic_w = (ui.available_width() - 95.0).max(120.0);
            ui.add_sized(
                [max_topic_w, 18.0],
                egui::Label::new(
                    RichText::new(&node.full_path)
                        .monospace()
                        .strong()
                        .size(14.0)
                        .color(Color32::from_rgb(130, 205, 255)),
                )
                .truncate(),
            )
            .on_hover_text(&node.full_path);
            if ui.small_button("Copy Topic").clicked() {
                ui.ctx().copy_text(node.full_path.clone());
            }
        });

        let hist_len = node.history.len();
        if hist_len == 0 {
            ui.label(format!(
                "Folder node ({} sub-topics, {} total messages in subtree).",
                node.children.len(),
                node.total_subtree_messages
            ));
            return;
        }

        if self.selected_history_idx >= hist_len {
            self.selected_history_idx = 0;
        }
        let total_topic_msgs = node.message_count;
        let msg = &node.history[self.selected_history_idx];
        let numeric_points: Vec<(f64, f64)> = if self.inspector_tab == InspectorTab::HistoryChart {
            node.numeric_history.iter().copied().collect()
        } else {
            Vec::new()
        };

        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(msg.timestamp.format("%H:%M:%S%.3f").to_string())
                    .monospace()
                    .color(Color32::from_rgb(180, 190, 205)),
            );
            ui.separator();
            ui.label(RichText::new(msg.qos.short_label()).strong());
            if msg.retain {
                ui.label(
                    RichText::new("RETAINED")
                        .color(Color32::from_rgb(245, 185, 75))
                        .strong(),
                );
            }
            ui.separator();
            ui.label(format!("{} bytes", msg.payload.len()));
            ui.separator();
            ui.label(format!("Total: {} msgs", total_topic_msgs));

            if hist_len > 1 {
                ui.separator();
                ui.label("History:");
                egui::ComboBox::from_id_salt("history_version_select")
                    .selected_text(if self.selected_history_idx == 0 {
                        format!("Latest (#1 of {})", hist_len)
                    } else {
                        format!("#{} of {}", self.selected_history_idx + 1, hist_len)
                    })
                    .show_ui(ui, |ui| {
                        for i in 0..hist_len {
                            let ts = node.history[i].timestamp.format("%H:%M:%S%.3f");
                            let lbl = if i == 0 {
                                format!("Latest ({})", ts)
                            } else {
                                format!("-{} ({})", i, ts)
                            };
                            ui.selectable_value(&mut self.selected_history_idx, i, lbl);
                        }
                    });
            }
        });

        if !msg.properties.is_empty() {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(26, 32, 44))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("MQTT v5 Properties:")
                                .color(Color32::from_rgb(130, 200, 255))
                                .strong()
                                .size(11.5),
                        );
                        if let Some(ref ct) = msg.properties.content_type {
                            ui.label(
                                RichText::new(format!("Content-Type: {}", ct))
                                    .monospace()
                                    .size(11.5),
                            );
                        }
                        if let Some(exp) = msg.properties.message_expiry_interval {
                            ui.label(
                                RichText::new(format!("Expiry: {}s", exp))
                                    .monospace()
                                    .size(11.5),
                            );
                        }
                        if let Some(ref rt) = msg.properties.response_topic {
                            ui.label(
                                RichText::new(format!("Response-Topic: {}", rt))
                                    .monospace()
                                    .size(11.5),
                            );
                        }
                        if let Some(ref cd) = msg.properties.correlation_data {
                            let cd_str = std::str::from_utf8(cd)
                                .map(|s| s.to_string())
                                .unwrap_or_else(|_| format!("0x{}", hex::encode(cd)));
                            ui.label(
                                RichText::new(format!("Correlation-Data: {}", cd_str))
                                    .monospace()
                                    .size(11.5),
                            );
                        }
                        for (k, v) in &msg.properties.user_properties {
                            ui.label(
                                RichText::new(format!("[{}: {}]", k, v))
                                    .monospace()
                                    .size(11.5)
                                    .color(Color32::from_rgb(175, 230, 175)),
                            );
                        }
                    });
                });
        }

        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.inspector_tab, InspectorTab::Payload, "Payload");
            ui.selectable_value(
                &mut self.inspector_tab,
                InspectorTab::Diff,
                format!("Diff View ({})", hist_len),
            );
            ui.selectable_value(
                &mut self.inspector_tab,
                InspectorTab::HistoryChart,
                "Value Chart",
            );

            ui.separator();
            ui.label("Format:");
            for fmt in [
                PayloadFormat::Auto,
                PayloadFormat::Json,
                PayloadFormat::Text,
                PayloadFormat::Hex,
                PayloadFormat::Base64,
                PayloadFormat::MessagePack,
                PayloadFormat::Protobuf,
            ] {
                ui.selectable_value(&mut self.payload_format, fmt, fmt.label());
            }
        });

        ui.separator();

        let cache_hit = self.inspector_cache.as_ref().is_some_and(|c| {
            c.topic == topic_path
                && c.history_idx == self.selected_history_idx
                && c.message_count == total_topic_msgs
                && c.format == self.payload_format
        });

        if !cache_hit {
            let formatted = format_payload(&msg.payload, self.payload_format);
            let json_job = if formatted.is_json_highlightable {
                Some(build_json_layout_job(&formatted.display_text))
            } else {
                None
            };
            self.inspector_cache = Some(InspectorCache {
                topic: topic_path.clone(),
                history_idx: self.selected_history_idx,
                message_count: total_topic_msgs,
                format: self.payload_format,
                formatted,
                json_job,
                diff_lines: None,
                diff_timestamps: None,
            });
        }

        if self.inspector_tab == InspectorTab::Diff {
            let needs_diff = self
                .inspector_cache
                .as_ref()
                .is_some_and(|c| c.diff_lines.is_none());
            if needs_diff && hist_len > 1 {
                let cmp_idx = (self.selected_history_idx + 1).min(hist_len - 1);
                if let Some(prev) = node.history.get(cmp_idx) {
                    let prev_formatted = format_payload(&prev.payload, self.payload_format);
                    let cur_ts = msg.timestamp.format("%H:%M:%S%.3f").to_string();
                    let prev_ts = prev.timestamp.format("%H:%M:%S%.3f").to_string();
                    if let Some(cache) = self.inspector_cache.as_mut() {
                        cache.diff_lines = Some(compute_payload_diff(
                            &prev_formatted.display_text,
                            &cache.formatted.display_text,
                        ));
                        cache.diff_timestamps = Some((cur_ts, prev_ts));
                    }
                }
            }
        }

        let cache = self.inspector_cache.as_ref().unwrap();
        let formatted = &cache.formatted;

        ScrollArea::both()
            .id_salt("inspector_body_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| match self.inspector_tab {
                InspectorTab::Payload => {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Detected / Active Format: {}",
                                formatted.detected_format.label()
                            ))
                            .size(11.5)
                            .color(Color32::from_rgb(145, 165, 190)),
                        );
                        if ui.small_button("Copy Formatted").clicked() {
                            ui.ctx().copy_text(formatted.display_text.clone());
                        }
                    });

                    if let Some(ref job) = cache.json_job {
                        ui.add(egui::Label::new(job.clone()).selectable(true));
                    } else {
                        ui.add(
                            egui::Label::new(
                                RichText::new(&formatted.display_text)
                                    .monospace()
                                    .size(12.5)
                                    .color(Color32::from_rgb(220, 228, 238)),
                            )
                            .selectable(true),
                        );
                    }
                }

                InspectorTab::Diff => {
                    if let (Some(diff_lines), Some((cur_ts, prev_ts))) =
                        (&cache.diff_lines, &cache.diff_timestamps)
                    {
                        ui.label(
                            RichText::new(format!(
                                "Comparing message at {} vs previous at {}",
                                cur_ts, prev_ts
                            ))
                            .size(11.5)
                            .color(Color32::from_rgb(155, 168, 185)),
                        );
                        ui.add_space(4.0);

                        for line in diff_lines {
                            let (prefix, color, bg) = match line.tag {
                                DiffTag::Equal => (
                                    "  ",
                                    Color32::from_rgb(195, 202, 215),
                                    Color32::TRANSPARENT,
                                ),
                                DiffTag::Delete => (
                                    "- ",
                                    Color32::from_rgb(255, 130, 130),
                                    Color32::from_rgba_unmultiplied(90, 25, 25, 90),
                                ),
                                DiffTag::Insert => (
                                    "+ ",
                                    Color32::from_rgb(125, 245, 150),
                                    Color32::from_rgba_unmultiplied(25, 85, 40, 90),
                                ),
                            };
                            egui::Frame::NONE.fill(bg).show(ui, |ui| {
                                ui.label(
                                    RichText::new(format!("{}{}", prefix, line.text))
                                        .monospace()
                                        .size(12.5)
                                        .color(color),
                                );
                            });
                        }
                    } else {
                        ui.label(
                            "Need at least 2 messages on this topic to compute a diff. Publish or receive another update on this topic.",
                        );
                    }
                }

                InspectorTab::HistoryChart => {
                    render_sparkline_chart(ui, &numeric_points);
                }
            });
    }
}

/// Syntax-highlighted JSON builder using `egui::text::LayoutJob` (cached per message).
fn build_json_layout_job(json_text: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let font_id = FontId::new(12.5, FontFamily::Monospace);

    for line in json_text.lines() {
        if let Some(colon_pos) = line.find(':') {
            let (left, right) = line.split_at(colon_pos);
            if left.trim().starts_with('"') {
                job.append(
                    left,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color: Color32::from_rgb(130, 200, 255),
                        ..Default::default()
                    },
                );
                job.append(
                    ":",
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color: Color32::from_rgb(190, 195, 205),
                        ..Default::default()
                    },
                );
                let val_part = &right[1..];
                let trimmed = val_part.trim().trim_end_matches(',');
                let val_color = if trimmed.starts_with('"') {
                    Color32::from_rgb(165, 230, 155)
                } else if trimmed == "true" || trimmed == "false" {
                    Color32::from_rgb(245, 175, 110)
                } else if trimmed == "null" {
                    Color32::from_rgb(160, 160, 175)
                } else {
                    Color32::from_rgb(235, 210, 120)
                };
                job.append(
                    val_part,
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color: val_color,
                        ..Default::default()
                    },
                );
                job.append(
                    "\n",
                    0.0,
                    TextFormat {
                        font_id: font_id.clone(),
                        color: Color32::WHITE,
                        ..Default::default()
                    },
                );
                continue;
            }
        }
        job.append(
            line,
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: Color32::from_rgb(205, 212, 224),
                ..Default::default()
            },
        );
        job.append(
            "\n",
            0.0,
            TextFormat {
                font_id: font_id.clone(),
                color: Color32::WHITE,
                ..Default::default()
            },
        );
    }

    job
}
