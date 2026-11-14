//! Throughput and root-topic distribution charts, plus the value sparkline
//! used by the inspector's History Chart tab.

use crate::tree::RootTopicStat;
use crate::ui::format_bytes;
use crate::MqttApp;
use eframe::egui::{
    self, Color32, FontFamily, FontId, Pos2, RichText, ScrollArea, Shape, Stroke, Ui,
};

const PIE_PALETTE: [Color32; 8] = [
    Color32::from_rgb(56, 189, 248),  // Cyan
    Color32::from_rgb(52, 211, 153),  // Emerald
    Color32::from_rgb(251, 191, 36),  // Amber
    Color32::from_rgb(167, 139, 250), // Violet
    Color32::from_rgb(248, 113, 113), // Rose
    Color32::from_rgb(96, 165, 250),  // Blue
    Color32::from_rgb(244, 114, 182), // Pink
    Color32::from_rgb(163, 230, 53),  // Lime
];

impl MqttApp {
    pub(crate) fn render_throughput_collapsing(&mut self, ui: &mut Ui) {
        let mpm = self.tree.messages_per_minute();
        let mps = self.tree.messages_per_second();
        let peak_mpm = self.tree.peak_msgs_per_min;
        let bytes_60s = self.tree.bytes_last_60s();
        let series = self.tree.series_last_60s(30);

        let header_resp = egui::CollapsingHeader::new(
            RichText::new(format!("Throughput ({:.0} msg/min)", mpm)).strong(),
        )
        .id_salt("throughput_collapsing")
        .default_open(true)
        .open(Some(self.show_throughput_section))
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(format!("{:.1} msg/s", mps))
                        .size(11.0)
                        .color(Color32::from_rgb(180, 200, 220)),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("Peak: {:.0}/m", peak_mpm))
                        .size(11.0)
                        .color(Color32::from_rgb(145, 165, 185)),
                );
                ui.separator();
                ui.label(
                    RichText::new(format!("{}/min", format_bytes(bytes_60s)))
                        .size(11.0)
                        .color(Color32::from_rgb(135, 205, 250)),
                );
            });
            ui.add_space(4.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), 112.0), |ui| {
                render_throughput_bar_chart(ui, &series);
            });
        });
        if header_resp.header_response.clicked() {
            self.show_throughput_section = !self.show_throughput_section;
        }
    }

    pub(crate) fn render_root_topics_collapsing(&mut self, ui: &mut Ui) {
        let root_stats = self.tree.root_topic_stats();
        let total_msgs = self.tree.total_messages_received;

        let header_resp = egui::CollapsingHeader::new(
            RichText::new(format!(
                "Root Topics ({} roots · {} msgs)",
                root_stats.len(),
                total_msgs
            ))
            .strong(),
        )
        .id_salt("root_topics_collapsing")
        .default_open(true)
        .open(Some(self.show_root_topics_section))
        .show(ui, |ui| {
            ui.allocate_ui(egui::vec2(ui.available_width(), 132.0), |ui| {
                render_root_topics_chart(ui, &root_stats, total_msgs);
            });
        });
        if header_resp.header_response.clicked() {
            self.show_root_topics_section = !self.show_root_topics_section;
        }
    }
}

fn render_root_topics_chart(ui: &mut Ui, stats: &[RootTopicStat], total_msgs: u64) {
    if stats.is_empty() || total_msgs == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("No topic data yet — click '+ Sample Data' or connect to a broker")
                    .size(11.5)
                    .color(Color32::from_rgb(130, 140, 155)),
            );
        });
        return;
    }

    let full_w = ui.available_width().max(120.0);

    // 1. Top Stacked Distribution Bar (GitHub / Grafana style)
    let (bar_rect, bar_resp) =
        ui.allocate_exact_size(egui::vec2(full_w, 10.0), egui::Sense::hover());
    let bar_painter = ui.painter_at(bar_rect);
    bar_painter.rect_filled(bar_rect, 4.0, Color32::from_rgb(24, 28, 38));

    let mut cur_x = bar_rect.left();
    let total_w = bar_rect.width();
    for (i, s) in stats.iter().enumerate() {
        let color = if i < 7 {
            PIE_PALETTE[i % PIE_PALETTE.len()]
        } else {
            Color32::from_rgb(120, 130, 148)
        };
        let seg_w = (s.fraction * total_w).max(if s.messages > 0 { 2.0 } else { 0.0 });
        let seg_right = (cur_x + seg_w).min(bar_rect.right());
        if seg_right > cur_x {
            let seg_rect = egui::Rect::from_min_max(
                Pos2::new(cur_x, bar_rect.top()),
                Pos2::new(seg_right, bar_rect.bottom()),
            );
            bar_painter.rect_filled(seg_rect, 2.0, color);
        }
        cur_x = seg_right + 1.0;
        if cur_x >= bar_rect.right() {
            break;
        }
    }

    if bar_resp.hovered() {
        let tip: Vec<String> = stats
            .iter()
            .take(10)
            .map(|s| {
                format!(
                    "{}: {} msgs ({:.1}%, {})",
                    s.name,
                    s.messages,
                    s.fraction * 100.0,
                    format_bytes(s.bytes)
                )
            })
            .collect();
        bar_resp.on_hover_text(tip.join("\n"));
    }

    ui.add_space(6.0);

    // 2. Scrollable Ranked Horizontal Progress Rows (100% deterministic painter layout)
    let list_h = ui.available_height().max(70.0);
    ui.allocate_ui(egui::vec2(full_w, list_h), |ui| {
        ScrollArea::vertical()
            .id_salt("root_topics_bars_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let row_w = ui.available_width().max(100.0);
                let row_h = 20.0_f32;

                for (i, s) in stats.iter().enumerate() {
                    let color = if i < 7 {
                        PIE_PALETTE[i % PIE_PALETTE.len()]
                    } else {
                        Color32::from_rgb(120, 130, 148)
                    };

                    let (row_rect, row_resp) =
                        ui.allocate_exact_size(egui::vec2(row_w, row_h), egui::Sense::hover());
                    let p = ui.painter_at(row_rect);

                    // Subtle row background + hover highlight
                    let bg_col = if row_resp.hovered() {
                        Color32::from_rgb(28, 34, 46)
                    } else {
                        Color32::from_rgb(19, 23, 31)
                    };
                    p.rect_filled(row_rect, 4.0, bg_col);

                    // Proportional progress fill
                    let fill_w = (s.fraction * row_rect.width()).max(3.0);
                    let fill_rect = egui::Rect::from_min_max(
                        row_rect.left_top(),
                        Pos2::new(
                            (row_rect.left() + fill_w).min(row_rect.right()),
                            row_rect.bottom(),
                        ),
                    );
                    p.rect_filled(
                        fill_rect,
                        4.0,
                        Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 42),
                    );

                    // Left color accent bar
                    let accent_rect = egui::Rect::from_min_max(
                        row_rect.left_top(),
                        Pos2::new(row_rect.left() + 3.0, row_rect.bottom()),
                    );
                    p.rect_filled(accent_rect, 2.0, color);

                    // Right-aligned metrics string
                    let metrics_str = format!(
                        "{:>5.1}% · {} ({})",
                        s.fraction * 100.0,
                        s.messages,
                        format_bytes(s.bytes)
                    );
                    let metrics_galley = p.layout_no_wrap(
                        metrics_str,
                        FontId::new(10.5, FontFamily::Monospace),
                        Color32::from_rgb(175, 190, 210),
                    );
                    let metrics_w = metrics_galley.size().x;
                    let metrics_pos = Pos2::new(
                        row_rect.right() - 8.0 - metrics_w,
                        row_rect.center().y - metrics_galley.size().y * 0.5,
                    );
                    p.galley(metrics_pos, metrics_galley, Color32::WHITE);

                    // Left-aligned root topic name, clipped cleanly before metrics_pos
                    let name_left = row_rect.left() + 10.0;
                    let max_name_w = (metrics_pos.x - name_left - 8.0).max(20.0);
                    let name_clip = egui::Rect::from_min_max(
                        Pos2::new(name_left, row_rect.top()),
                        Pos2::new(name_left + max_name_w, row_rect.bottom()),
                    );
                    let name_painter = ui.painter_at(name_clip);
                    let name_galley = name_painter.layout_no_wrap(
                        s.name.clone(),
                        FontId::new(11.5, FontFamily::Monospace),
                        Color32::from_rgb(230, 236, 248),
                    );
                    let name_pos =
                        Pos2::new(name_left, row_rect.center().y - name_galley.size().y * 0.5);
                    name_painter.galley(name_pos, name_galley, Color32::WHITE);

                    if row_resp.hovered() {
                        row_resp.on_hover_text(format!(
                            "Root topic: {}\nMessages: {} ({:.1}%)\nVolume: {}",
                            s.name,
                            s.messages,
                            s.fraction * 100.0,
                            format_bytes(s.bytes)
                        ));
                    }
                }
            });
    });
}

fn render_throughput_bar_chart(ui: &mut Ui, series: &[(u64, u64)]) {
    let desired = egui::vec2(ui.available_width(), ui.available_height().max(85.0));
    let (rect, resp) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(13, 16, 22));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_rgb(38, 46, 60)),
        egui::StrokeKind::Inside,
    );

    let pad_x = 8.0_f32;
    let pad_top = 8.0_f32;
    let pad_bottom = 16.0_f32;
    let plot = egui::Rect::from_min_max(
        Pos2::new(rect.left() + pad_x, rect.top() + pad_top),
        Pos2::new(rect.right() - pad_x, rect.bottom() - pad_bottom),
    );

    // Subtle horizontal grid lines
    for frac in [0.25_f32, 0.5, 0.75] {
        let y = plot.top() + plot.height() * frac;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(1.0_f32, Color32::from_rgb(24, 30, 40)),
        );
    }

    let max_msgs = series.iter().map(|s| s.0).max().unwrap_or(1).max(1) as f32;
    let n = series.len().max(1);
    let step_w = plot.width() / (n as f32);
    let bar_w = (step_w * 0.72).max(2.0);

    let mut line_pts = Vec::with_capacity(n);

    for (i, &(msgs, _bytes)) in series.iter().enumerate() {
        let x_left = plot.left() + (i as f32) * step_w + (step_w - bar_w) * 0.5;
        let norm = (msgs as f32 / max_msgs).clamp(0.0, 1.0);
        let bar_h = (norm * plot.height()).max(if msgs > 0 { 3.0 } else { 0.0 });
        let bar_rect = egui::Rect::from_min_max(
            Pos2::new(x_left, plot.bottom() - bar_h),
            Pos2::new(x_left + bar_w, plot.bottom()),
        );

        if bar_h > 0.0 {
            painter.rect_filled(
                bar_rect,
                2.0,
                Color32::from_rgba_unmultiplied(45, 195, 145, 175),
            );
        }

        let cx = x_left + bar_w * 0.5;
        let cy = plot.bottom() - bar_h;
        line_pts.push(Pos2::new(cx, cy));
    }

    if line_pts.len() >= 2 {
        painter.add(Shape::line(
            line_pts,
            Stroke::new(1.6_f32, Color32::from_rgb(80, 215, 255)),
        ));
    }

    // X-axis time labels
    painter.text(
        Pos2::new(plot.left(), rect.bottom() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        "-60s",
        FontId::new(10.0, FontFamily::Monospace),
        Color32::from_rgb(125, 135, 152),
    );
    painter.text(
        Pos2::new(plot.center().x, rect.bottom() - 2.0),
        egui::Align2::CENTER_BOTTOM,
        "-30s",
        FontId::new(10.0, FontFamily::Monospace),
        Color32::from_rgb(125, 135, 152),
    );
    painter.text(
        Pos2::new(plot.right(), rect.bottom() - 2.0),
        egui::Align2::RIGHT_BOTTOM,
        "Now",
        FontId::new(10.0, FontFamily::Monospace),
        Color32::from_rgb(125, 135, 152),
    );

    if resp.hovered() {
        let total_60s: u64 = series.iter().map(|s| s.0).sum();
        resp.on_hover_text(format!(
            "Rolling 60s window (2s buckets)\nMessages in last 60s: {}\nMax bucket: {:.0} msgs / 2s",
            total_60s, max_msgs
        ));
    }
}

pub(crate) fn render_sparkline_chart(ui: &mut Ui, points: &[(f64, f64)]) {
    if points.is_empty() {
        ui.label("No numeric values extracted on this topic yet.");
        return;
    }

    let min_v = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let max_v = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    let sum_v: f64 = points.iter().map(|p| p.1).sum();
    let avg_v = sum_v / (points.len() as f64);
    let latest_v = points.last().map(|p| p.1).unwrap_or(0.0);

    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("Latest: {:.2}", latest_v)).strong());
        ui.separator();
        ui.label(format!("Min: {:.2}", min_v));
        ui.separator();
        ui.label(format!("Max: {:.2}", max_v));
        ui.separator();
        ui.label(format!("Avg: {:.2}", avg_v));
        ui.separator();
        ui.label(format!("Samples: {}", points.len()));
    });

    ui.add_space(6.0);
    let desired_size = egui::vec2(ui.available_width(), 160.0);
    let (rect, _resp) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(15, 18, 24));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_rgb(45, 55, 70)),
        egui::StrokeKind::Inside,
    );

    if points.len() < 2 {
        return;
    }

    let span = (max_v - min_v).abs().max(1e-6);
    let pad = 12.0;
    let plot_rect = rect.shrink(pad);

    let screen_pts: Vec<egui::Pos2> = points
        .iter()
        .enumerate()
        .map(|(i, &(_, val))| {
            let x_norm = i as f32 / ((points.len() - 1) as f32);
            let y_norm = ((val - min_v) / span) as f32;
            let x = plot_rect.left() + x_norm * plot_rect.width();
            let y = plot_rect.bottom() - y_norm * plot_rect.height();
            egui::pos2(x, y)
        })
        .collect();

    painter.add(egui::Shape::line(
        screen_pts.clone(),
        Stroke::new(2.0_f32, Color32::from_rgb(85, 195, 255)),
    ));

    for pt in screen_pts {
        painter.circle_filled(pt, 3.0, Color32::from_rgb(130, 235, 170));
    }
}
