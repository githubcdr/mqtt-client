//! Central topic tree explorer panel and recursive node rendering.

use crate::tree::TopicNode;
use crate::ui::draw_status_dot;
use crate::MqttApp;
use eframe::egui::{self, Color32, Pos2, RichText, ScrollArea, Shape, Stroke, Ui};
use std::time::Instant;

impl MqttApp {
    pub(crate) fn render_topic_tree_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Topic Explorer").size(15.0).strong());
            ui.separator();
            ui.label("Filter:");
            ui.add(
                egui::TextEdit::singleline(&mut self.search_filter)
                    .hint_text("Filter topics or payload values...")
                    .desired_width(200.0),
            );
            if !self.search_filter.is_empty() && ui.small_button("x").clicked() {
                self.search_filter.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Clear").clicked() {
                    self.tree.clear();
                    self.selected_topic = None;
                }
                let pause_txt = if self.paused { "Resume" } else { "Pause" };
                if ui.button(pause_txt).clicked() {
                    self.paused = !self.paused;
                }
                if ui.button("- Collapse All").clicked() {
                    self.tree.root.set_expanded_recursive(false);
                }
                if ui.button("+ Expand All").clicked() {
                    self.tree.root.set_expanded_recursive(true);
                }
            });
        });

        ui.separator();

        if self.tree.root.children.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.label(
                    RichText::new("No MQTT messages in topic tree yet.")
                        .size(16.0)
                        .color(Color32::from_rgb(140, 150, 168)),
                );
                ui.add_space(8.0);
                ui.label(
                    "Connect to a broker or click '+ Sample Data' in the top-right to explore.",
                );
                ui.add_space(12.0);
                if ui.button("+ Generate Sample MQTT Traffic Now").clicked() {
                    self.inject_sample_traffic();
                }
            });
            return;
        }

        let query_lower = self.search_filter.trim().to_lowercase();
        let now = Instant::now();
        let mut clicked_topic: Option<String> = None;
        let selected_clone = self.selected_topic.clone();

        ScrollArea::both()
            .id_salt("topic_tree_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for child in self.tree.root.children.values_mut() {
                    Self::render_tree_node(
                        ui,
                        child,
                        0,
                        &query_lower,
                        now,
                        selected_clone.as_deref(),
                        &mut clicked_topic,
                    );
                }
            });

        if let Some(topic) = clicked_topic {
            self.selected_topic = Some(topic);
            self.selected_history_idx = 0;
            self.compare_history_idx = 1;
        }
    }

    fn render_tree_node(
        ui: &mut Ui,
        node: &mut TopicNode,
        depth: usize,
        query_lower: &str,
        now: Instant,
        selected_topic: Option<&str>,
        clicked_topic: &mut Option<String>,
    ) {
        if !query_lower.is_empty() && !node.matches_filter(query_lower) {
            return;
        }

        let has_children = !node.children.is_empty();
        let is_selected = selected_topic == Some(node.full_path.as_str());
        let recently_updated = node
            .last_updated
            .map(|t| now.duration_since(t).as_millis() < 1800)
            .unwrap_or(false);

        ui.horizontal(|ui| {
            ui.add_space((depth as f32) * 16.0);

            if has_children {
                if draw_chevron_button(ui, node.expanded) {
                    node.expanded = !node.expanded;
                }
            } else {
                ui.add_space(18.0);
            }

            if recently_updated {
                draw_status_dot(ui, 3.5, Color32::from_rgb(80, 240, 130));
            }

            let seg_color = if is_selected {
                Color32::from_rgb(255, 225, 110)
            } else if has_children {
                Color32::from_rgb(135, 195, 255)
            } else {
                Color32::from_rgb(225, 232, 242)
            };

            let resp = ui.selectable_label(
                is_selected,
                RichText::new(&node.segment).color(seg_color).strong(),
            );
            if resp.clicked() {
                if node.message_count > 0 {
                    *clicked_topic = Some(node.full_path.clone());
                } else if has_children {
                    node.expanded = !node.expanded;
                    *clicked_topic = Some(node.full_path.clone());
                }
            }

            if has_children {
                ui.label(
                    RichText::new(format!(
                        "({} topics, {} msgs)",
                        node.children.len(),
                        node.total_subtree_messages
                    ))
                    .size(11.0)
                    .color(Color32::from_rgb(125, 135, 152)),
                );
            }

            if let Some(latest) = node.latest_message() {
                if latest.retain {
                    ui.label(
                        RichText::new("RETAIN")
                            .size(10.0)
                            .color(Color32::from_rgb(245, 185, 75))
                            .strong(),
                    );
                }
                ui.label(
                    RichText::new(format!("= {}", latest.compact_preview))
                        .monospace()
                        .size(12.0)
                        .color(Color32::from_rgb(165, 220, 175)),
                );
            }
        });

        if has_children && (node.expanded || !query_lower.is_empty()) {
            for child in node.children.values_mut() {
                Self::render_tree_node(
                    ui,
                    child,
                    depth + 1,
                    query_lower,
                    now,
                    selected_topic,
                    clicked_topic,
                );
            }
        }
    }
}

/// Paint a crisp clickable expand/collapse triangle for topic tree nodes.
fn draw_chevron_button(ui: &mut Ui, expanded: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 3.0, Color32::from_rgb(42, 48, 62));
    }
    let c = rect.center();
    let color = if resp.hovered() {
        Color32::from_rgb(230, 238, 250)
    } else {
        Color32::from_rgb(160, 172, 192)
    };
    let pts = if expanded {
        vec![
            Pos2::new(c.x - 4.0, c.y - 2.0),
            Pos2::new(c.x + 4.0, c.y - 2.0),
            Pos2::new(c.x, c.y + 3.5),
        ]
    } else {
        vec![
            Pos2::new(c.x - 2.0, c.y - 4.0),
            Pos2::new(c.x + 3.5, c.y),
            Pos2::new(c.x - 2.0, c.y + 4.0),
        ]
    };
    ui.painter()
        .add(Shape::convex_polygon(pts, color, Stroke::NONE));
    resp.clicked()
}
