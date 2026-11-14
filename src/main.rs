#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod client;
mod config;
mod icon;
mod payload;
mod protocol;
mod tree;

use client::{spawn_mqtt_session, ClientCommand, ClientEvent};
use config::{uuid_short, AppConfig, ConnectionProfile, SubscriptionRule};
use eframe::egui::{
    self, Color32, FontFamily, FontId, Pos2, RichText, ScrollArea, Shape, Stroke, TextFormat, Ui,
};
use payload::{encode_publish_payload, format_payload, PayloadFormat, PublishEncoding};
use protocol::{Mqtt5Properties, MqttVersion, QoS};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tree::{compute_payload_diff, DiffTag, RootTopicStat, TopicNode, TopicTree};

fn main() -> eframe::Result<()> {
    let app_icon = Arc::new(icon::build_window_icon());
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(concat!(
                "MQTT Explorer RS v",
                env!("CARGO_PKG_VERSION"),
                " — Lightweight MQTT 3.1 / 3.1.1 / 5.0 Client"
            ))
            .with_app_id("mqtt-client")
            .with_inner_size([1380.0, 880.0])
            .with_min_inner_size([980.0, 640.0])
            .with_icon(app_icon),
        ..Default::default()
    };

    eframe::run_native(
        concat!("MQTT Explorer RS v", env!("CARGO_PKG_VERSION")),
        native_options,
        Box::new(|cc| Ok(Box::new(MqttApp::new(cc)))),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Payload,
    Diff,
    HistoryChart,
}

struct PublishDraft {
    topic: String,
    payload_input: String,
    encoding: PublishEncoding,
    qos: QoS,
    retain: bool,
    show_v5_props: bool,
    content_type: String,
    response_topic: String,
    correlation_data: String,
    message_expiry: String,
    payload_format_utf8: bool,
    user_properties: Vec<(String, String)>,
}

impl Default for PublishDraft {
    fn default() -> Self {
        Self {
            topic: "sensors/factory/line-1/telemetry".to_string(),
            payload_input: "{\n  \"device\": \"sensor-01\",\n  \"temperature\": 23.4,\n  \"humidity\": 48.2,\n  \"status\": \"ok\"\n}".to_string(),
            encoding: PublishEncoding::Json,
            qos: QoS::AtMostOnce,
            retain: false,
            show_v5_props: false,
            content_type: "application/json".to_string(),
            response_topic: String::new(),
            correlation_data: String::new(),
            message_expiry: String::new(),
            payload_format_utf8: true,
            user_properties: Vec::new(),
        }
    }
}

struct InspectorCache {
    topic: String,
    history_idx: usize,
    message_count: u64,
    format: PayloadFormat,
    formatted: payload::FormattedPayload,
    json_job: Option<egui::text::LayoutJob>,
    diff_lines: Option<Vec<tree::DiffLine>>,
    diff_timestamps: Option<(String, String)>,
}

struct MqttApp {
    config: AppConfig,
    show_profile_sidebar: bool,
    show_performance_panel: bool,
    show_root_topics_section: bool,
    show_throughput_section: bool,
    show_password: bool,
    new_sub_filter: String,
    new_sub_qos: QoS,

    conn_state: ConnectionState,
    active_version: MqttVersion,
    cmd_tx: Option<mpsc::UnboundedSender<ClientCommand>>,
    event_rx: Option<mpsc::UnboundedReceiver<ClientEvent>>,

    tree: TopicTree,
    paused: bool,
    search_filter: String,
    selected_topic: Option<String>,
    selected_history_idx: usize,
    compare_history_idx: usize,

    inspector_tab: InspectorTab,
    payload_format: PayloadFormat,
    inspector_cache: Option<InspectorCache>,
    publish_draft: PublishDraft,
    show_publish_drawer: bool,

    published_count: u64,
    published_bytes: u64,
    logs: VecDeque<String>,
    show_logs: bool,
    demo_counter: u64,
}

impl MqttApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut visuals = egui::Visuals::dark();
        visuals.window_fill = Color32::from_rgb(18, 20, 26);
        visuals.panel_fill = Color32::from_rgb(22, 25, 32);
        visuals.faint_bg_color = Color32::from_rgb(28, 32, 42);
        visuals.extreme_bg_color = Color32::from_rgb(13, 15, 20);
        cc.egui_ctx.set_visuals(visuals);

        let config = AppConfig::load_or_default();
        let active_version = config
            .profiles
            .get(config.selected_profile_idx)
            .map(|p| p.version)
            .unwrap_or(MqttVersion::V5);

        let mut app = Self {
            config,
            show_profile_sidebar: true,
            show_performance_panel: true,
            show_root_topics_section: true,
            show_throughput_section: true,
            show_password: false,
            new_sub_filter: String::new(),
            new_sub_qos: QoS::AtMostOnce,
            conn_state: ConnectionState::Disconnected,
            active_version,
            cmd_tx: None,
            event_rx: None,
            tree: TopicTree::new(),
            paused: false,
            search_filter: String::new(),
            selected_topic: None,
            selected_history_idx: 0,
            compare_history_idx: 1,
            inspector_tab: InspectorTab::Payload,
            payload_format: PayloadFormat::Auto,
            inspector_cache: None,
            publish_draft: PublishDraft::default(),
            show_publish_drawer: true,
            published_count: 0,
            published_bytes: 0,
            logs: VecDeque::new(),
            show_logs: false,
            demo_counter: 0,
        };
        app.log("Ready. Select a profile and click Connect, or click 'Sample Data' to preview.");
        app
    }

    fn log(&mut self, msg: impl Into<String>) {
        let now = chrono::Local::now().format("%H:%M:%S");
        self.logs.push_front(format!("[{}] {}", now, msg.into()));
        while self.logs.len() > 200 {
            self.logs.pop_back();
        }
    }

    fn current_profile(&self) -> &ConnectionProfile {
        let idx = self
            .config
            .selected_profile_idx
            .min(self.config.profiles.len().saturating_sub(1));
        &self.config.profiles[idx]
    }

    fn current_profile_mut(&mut self) -> &mut ConnectionProfile {
        if self.config.profiles.is_empty() {
            self.config.profiles.push(ConnectionProfile::default());
        }
        let idx = self
            .config
            .selected_profile_idx
            .min(self.config.profiles.len() - 1);
        &mut self.config.profiles[idx]
    }

    fn connect_selected_profile(&mut self) {
        self.disconnect();
        let profile = self.current_profile().clone();
        let _ = self.config.save_to_file(&AppConfig::default_config_path());

        self.active_version = profile.version;
        self.conn_state = ConnectionState::Connecting;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let cmd_tx = spawn_mqtt_session(profile, event_tx);
        self.cmd_tx = Some(cmd_tx);
        self.event_rx = Some(event_rx);
    }

    fn disconnect(&mut self) {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(ClientCommand::Disconnect);
        }
        self.event_rx = None;
        self.conn_state = ConnectionState::Disconnected;
    }

    fn poll_events(&mut self) {
        let mut pending = Vec::new();
        if let Some(rx) = &mut self.event_rx {
            while let Ok(ev) = rx.try_recv() {
                pending.push(ev);
            }
        }

        for ev in pending {
            match ev {
                ClientEvent::Connecting {
                    host,
                    port,
                    tls,
                    version,
                } => {
                    self.conn_state = ConnectionState::Connecting;
                    self.log(format!(
                        "Connecting to {}:{} ({}, TLS={})...",
                        host,
                        port,
                        version.label(),
                        tls
                    ));
                }
                ClientEvent::Connected {
                    session_present,
                    assigned_client_id,
                    server_keep_alive,
                } => {
                    self.conn_state = ConnectionState::Connected;
                    let mut detail = format!("Connected (session_present={})", session_present);
                    if let Some(cid) = assigned_client_id {
                        detail.push_str(&format!(", assigned_id={}", cid));
                    }
                    if let Some(ka) = server_keep_alive {
                        detail.push_str(&format!(", keep_alive={}s", ka));
                    }
                    self.log(detail);
                }
                ClientEvent::MessageReceived {
                    topic,
                    payload,
                    qos,
                    retain,
                    properties,
                } => {
                    if !self.paused {
                        if self.selected_topic.is_none() {
                            self.selected_topic = Some(topic.clone());
                        }
                        self.tree.insert_message(
                            topic,
                            payload,
                            qos,
                            retain,
                            properties,
                            self.config.max_history_per_topic,
                        );
                    }
                }
                ClientEvent::MessagePublished { topic, bytes } => {
                    self.published_count += 1;
                    self.published_bytes += bytes as u64;
                    self.log(format!("Published {} bytes to '{}'", bytes, topic));
                }
                ClientEvent::Subscribed { topics } => {
                    self.log(format!("Subscribed to: {}", topics.join(", ")));
                }
                ClientEvent::Unsubscribed { topics } => {
                    self.log(format!("Unsubscribed from: {}", topics.join(", ")));
                }
                ClientEvent::Log(m) => self.log(m),
                ClientEvent::Error(e) => self.log(format!("ERROR: {}", e)),
                ClientEvent::Disconnected(reason) => {
                    self.conn_state = ConnectionState::Disconnected;
                    self.cmd_tx = None;
                    self.event_rx = None;
                    self.log(format!("Disconnected: {}", reason));
                }
            }
        }
    }

    fn inject_sample_traffic(&mut self) {
        self.demo_counter += 1;
        let step = self.demo_counter;
        let temp = 21.5 + ((step as f64 * 0.7).sin() * 3.2 * 10.0).round() / 10.0;
        let hum = 45.0 + ((step as f64 * 0.5).cos() * 6.0 * 10.0).round() / 10.0;

        // 1. Root topic: `sensors/...` (JSON telemetry with MQTT v5 properties + scalar)
        let mut props = Mqtt5Properties::default();
        props.content_type = Some("application/json".to_string());
        props.response_topic = Some("sensors/factory/line-1/cmd-ack".to_string());
        props.correlation_data = Some(format!("req-{:04}", step).into_bytes());
        props.message_expiry_interval = Some(300);
        props.payload_format_indicator = Some(1);
        props.user_properties = vec![
            ("firmware".to_string(), "v2.4.1".to_string()),
            ("region".to_string(), "eu-central-1".to_string()),
        ];

        let json_payload = format!(
            "{{\n  \"device\": \"line-1-env\",\n  \"sequence\": {},\n  \"temperature\": {:.1},\n  \"humidity\": {:.1},\n  \"status\": \"online\"\n}}",
            step, temp, hum
        );
        self.tree.insert_message(
            "sensors/factory/line-1/telemetry".to_string(),
            json_payload.into_bytes(),
            QoS::AtLeastOnce,
            true,
            props,
            self.config.max_history_per_topic,
        );

        self.tree.insert_message(
            "sensors/factory/line-1/temperature".to_string(),
            format!("{:.2}", temp).into_bytes(),
            QoS::AtMostOnce,
            false,
            Mqtt5Properties::default(),
            self.config.max_history_per_topic,
        );

        // 2. Root topic: `factory/...` (Binary MessagePack + Protobuf Wire)
        let msgpack_json = format!(
            "{{\"motor_rpm\":{},\"voltage\":230.4,\"active\":true}}",
            1450 + (step % 15) * 10
        );
        if let Ok(mp_bytes) =
            encode_publish_payload(&msgpack_json, PublishEncoding::MessagePackFromJson)
        {
            self.tree.insert_message(
                "factory/line-2/msgpack-drive".to_string(),
                mp_bytes,
                QoS::AtLeastOnce,
                false,
                Mqtt5Properties::default(),
                self.config.max_history_per_topic,
            );
        }

        let pb_json = format!(
            "{{\"field_1\":{},\"field_2\":\"unit-alpha\",\"field_3\":{}}}",
            step,
            (temp * 10.0) as i64
        );
        if let Ok(pb_bytes) = encode_publish_payload(&pb_json, PublishEncoding::ProtobufFromJson) {
            self.tree.insert_message(
                "factory/line-2/protobuf-plc".to_string(),
                pb_bytes,
                QoS::ExactlyOnce,
                false,
                Mqtt5Properties::default(),
                self.config.max_history_per_topic,
            );
        }

        // 3. Root topic: `fleet/...` (GPS / vehicle telemetry)
        let fleet_json = format!(
            "{{\"vehicle\":\"agv-04\",\"battery_pct\":{},\"speed_mps\":{:.1}}}",
            98u64.saturating_sub(step % 40),
            1.4 + ((step % 5) as f64) * 0.2
        );
        self.tree.insert_message(
            "fleet/agv-04/position".to_string(),
            fleet_json.into_bytes(),
            QoS::AtMostOnce,
            false,
            Mqtt5Properties::default(),
            self.config.max_history_per_topic,
        );

        // 4. Root topic: `$SYS/...` (Broker health metrics)
        self.tree.insert_message(
            "$SYS/broker/load/messages/received/1min".to_string(),
            format!("{:.1}", self.tree.messages_per_minute() + 6.0).into_bytes(),
            QoS::AtMostOnce,
            true,
            Mqtt5Properties::default(),
            self.config.max_history_per_topic,
        );

        if self.selected_topic.is_none() {
            self.selected_topic = Some("sensors/factory/line-1/telemetry".to_string());
        }
        self.log(format!(
            "Injected sample batch #{} across 4 root topics (sensors, factory, fleet, $SYS)",
            step
        ));
    }

    fn execute_publish(&mut self) {
        let topic = self.publish_draft.topic.trim().to_string();
        if topic.is_empty() {
            self.log("ERROR: Publish topic cannot be empty.");
            return;
        }

        let payload = match encode_publish_payload(
            &self.publish_draft.payload_input,
            self.publish_draft.encoding,
        ) {
            Ok(b) => b,
            Err(e) => {
                self.log(format!("Publish encode error: {}", e));
                return;
            }
        };

        let mut props = Mqtt5Properties::default();
        if self.active_version == MqttVersion::V5 {
            if self.publish_draft.payload_format_utf8 {
                props.payload_format_indicator = Some(1);
            }
            if !self.publish_draft.content_type.trim().is_empty() {
                props.content_type = Some(self.publish_draft.content_type.trim().to_string());
            }
            if !self.publish_draft.response_topic.trim().is_empty() {
                props.response_topic = Some(self.publish_draft.response_topic.trim().to_string());
            }
            if !self.publish_draft.correlation_data.trim().is_empty() {
                props.correlation_data = Some(
                    self.publish_draft
                        .correlation_data
                        .trim()
                        .as_bytes()
                        .to_vec(),
                );
            }
            if let Ok(exp) = self.publish_draft.message_expiry.trim().parse::<u32>() {
                props.message_expiry_interval = Some(exp);
            }
            for (k, v) in &self.publish_draft.user_properties {
                if !k.trim().is_empty() {
                    props
                        .user_properties
                        .push((k.trim().to_string(), v.clone()));
                }
            }
        }

        let qos = self.publish_draft.qos;
        let retain = self.publish_draft.retain;

        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(ClientCommand::Publish {
                topic,
                payload,
                qos,
                retain,
                properties: props,
            });
        } else {
            let len = payload.len();
            self.tree.insert_message(
                topic.clone(),
                payload,
                qos,
                retain,
                props,
                self.config.max_history_per_topic,
            );
            self.published_count += 1;
            self.published_bytes += len as u64;
            if self.selected_topic.is_none() {
                self.selected_topic = Some(topic.clone());
            }
            self.log(format!(
                "Offline loopback publish: {} bytes -> '{}'",
                len, topic
            ));
        }
    }
}

impl eframe::App for MqttApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_events();

        if self.conn_state != ConnectionState::Disconnected {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
        }

        self.render_top_bar(ui);
        self.render_bottom_bar(ui);

        if self.show_profile_sidebar {
            egui::Panel::left("profile_sidebar")
                .resizable(true)
                .default_size(320.0)
                .size_range(260.0..=440.0)
                .show(ui, |ui| {
                    self.render_profile_sidebar(ui);
                });
        }

        egui::Panel::right("inspector_panel")
            .resizable(true)
            .default_size(480.0)
            .size_range(340.0..=820.0)
            .show(ui, |ui| {
                self.render_right_panel(ui);
            });

        egui::CentralPanel::default().show(ui, |ui| {
            self.render_topic_tree_panel(ui);
        });
    }
}

impl MqttApp {
    fn render_top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar")
            .exact_size(46.0)
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let sidebar_label = if self.show_profile_sidebar {
                        "◀ Profiles"
                    } else {
                        "▶ Profiles"
                    };
                    if ui.button(sidebar_label).clicked() {
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
                            RichText::new("🔒 TLS")
                                .color(Color32::from_rgb(110, 230, 150))
                                .strong(),
                        );
                    }

                    match self.conn_state {
                        ConnectionState::Disconnected => {
                            let btn = egui::Button::new(
                                RichText::new("⚡ Connect")
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
                                RichText::new("⏳ Cancel")
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
                                RichText::new("⏹ Disconnect")
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
                        let perf_btn_txt = if self.show_performance_panel {
                            "Hide Graphs"
                        } else {
                            "Show Graphs"
                        };
                        if ui.button(perf_btn_txt).clicked() {
                            self.show_performance_panel = !self.show_performance_panel;
                        }
                    });
                });
            });
    }

    fn render_bottom_bar(&mut self, ui: &mut egui::Ui) {
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

        panel.show(ui, |ui| {
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
        });
    }

    fn render_profile_sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading("Connection Profiles");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("💾 Save")
                    .on_hover_text("Save profiles to mqtt_profiles.toml")
                    .clicked()
                {
                    match self.config.save_to_file(&AppConfig::default_config_path()) {
                        Ok(_) => self.log("Saved profiles to mqtt_profiles.toml"),
                        Err(e) => self.log(e),
                    }
                }
                if ui
                    .small_button("📤 Export")
                    .on_hover_text("Export profiles to TOML or JSON file")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML / JSON Config", &["toml", "json"])
                        .set_file_name("mqtt_profiles.toml")
                        .save_file()
                    {
                        match self.config.save_to_file(&path) {
                            Ok(_) => self.log(format!("Exported config to {}", path.display())),
                            Err(e) => self.log(e),
                        }
                    }
                }
                if ui
                    .small_button("📥 Import")
                    .on_hover_text("Import profiles from TOML or JSON file")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("TOML / JSON Config", &["toml", "json"])
                        .pick_file()
                    {
                        match AppConfig::load_from_file(&path) {
                            Ok(loaded) => {
                                if !loaded.profiles.is_empty() {
                                    self.config = loaded;
                                    self.log(format!("Imported profiles from {}", path.display()));
                                }
                            }
                            Err(e) => self.log(e),
                        }
                    }
                }
            });
        });

        ui.horizontal(|ui| {
            if ui.button("➕ New Profile").clicked() {
                let mut p = ConnectionProfile::default();
                p.name = format!("Broker {}", self.config.profiles.len() + 1);
                self.config.profiles.push(p);
                self.config.selected_profile_idx = self.config.profiles.len() - 1;
            }
            if ui.button("📋 Duplicate").clicked() {
                let mut dup = self.current_profile().clone();
                dup.id = uuid_short();
                dup.name = format!("{} (Copy)", dup.name);
                dup.client_id = format!("mqtt-rs-{}", uuid_short());
                self.config.profiles.push(dup);
                self.config.selected_profile_idx = self.config.profiles.len() - 1;
            }
            if self.config.profiles.len() > 1 && ui.button("🗑 Delete").clicked() {
                self.config.profiles.remove(self.config.selected_profile_idx);
                if self.config.selected_profile_idx >= self.config.profiles.len() {
                    self.config.selected_profile_idx = self.config.profiles.len() - 1;
                }
            }
        });

        ui.separator();

        ScrollArea::vertical()
            .id_salt("profile_settings_scroll")
            .show(ui, |ui| {
                let mut show_pw = self.show_password;
                let profile = self.current_profile_mut();

                egui::Grid::new("broker_basic_grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .striped(false)
                    .show(ui, |ui| {
                        ui.label("Profile Name:");
                        ui.text_edit_singleline(&mut profile.name);
                        ui.end_row();

                        ui.label("Protocol:");
                        egui::ComboBox::from_id_salt("proto_ver_combo")
                            .selected_text(profile.version.label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut profile.version,
                                    MqttVersion::V31,
                                    MqttVersion::V31.label(),
                                );
                                ui.selectable_value(
                                    &mut profile.version,
                                    MqttVersion::V311,
                                    MqttVersion::V311.label(),
                                );
                                ui.selectable_value(
                                    &mut profile.version,
                                    MqttVersion::V5,
                                    MqttVersion::V5.label(),
                                );
                            });
                        ui.end_row();

                        ui.label("Host:");
                        ui.text_edit_singleline(&mut profile.host);
                        ui.end_row();

                        ui.label("Port:");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut profile.port).range(1..=65535));
                            if ui.small_button("1883").clicked() {
                                profile.port = 1883;
                                profile.tls.enabled = false;
                            }
                            if ui.small_button("8883 (TLS)").clicked() {
                                profile.port = 8883;
                                profile.tls.enabled = true;
                            }
                        });
                        ui.end_row();

                        ui.label("Client ID:");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut profile.client_id)
                                    .desired_width(145.0),
                            );
                            if ui
                                .small_button("Rand")
                                .on_hover_text("Generate random Client ID")
                                .clicked()
                            {
                                profile.client_id = format!("mqtt-rs-{}", uuid_short());
                            }
                        });
                        ui.end_row();

                        ui.label("Session:");
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut profile.clean_session, "Clean Start");
                            ui.label("KeepAlive:");
                            ui.add(
                                egui::DragValue::new(&mut profile.keep_alive_secs)
                                    .range(5..=3600)
                                    .suffix("s"),
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(6.0);
                egui::CollapsingHeader::new(
                    RichText::new("🔑 Authentication (Username / Password)").strong(),
                )
                .default_open(true)
                .show(ui, |ui| {
                    egui::Grid::new("auth_grid")
                        .num_columns(2)
                        .spacing([8.0, 6.0])
                        .show(ui, |ui| {
                            ui.label("Username:");
                            ui.text_edit_singleline(&mut profile.username);
                            ui.end_row();

                            ui.label("Password:");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut profile.password)
                                        .password(!show_pw)
                                        .desired_width(145.0),
                                );
                                if ui
                                    .small_button(if show_pw { "🙈" } else { "👁" })
                                    .clicked()
                                {
                                    show_pw = !show_pw;
                                }
                            });
                            ui.end_row();
                        });
                });
                self.show_password = show_pw;

                let profile = self.current_profile_mut();
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("🔒 TLS / Certificates (rustls)").strong())
                    .default_open(true)
                    .show(ui, |ui| {
                        ui.checkbox(&mut profile.tls.enabled, "Enable TLS / SSL (MQTT over TLS)");
                        if profile.tls.enabled {
                            ui.checkbox(
                                &mut profile.tls.accept_invalid_certs,
                                "Skip certificate verification (insecure / self-signed)",
                            );

                            ui.add_space(4.0);
                            ui.label("Custom CA Certificate (.pem / .crt / .der):");
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut profile.tls.ca_cert_path)
                                        .hint_text("Optional custom CA path...")
                                        .desired_width(190.0),
                                );
                                if ui.button("📂 Browse...").clicked() {
                                    if let Some(path) = rfd::FileDialog::new()
                                        .add_filter("Certificates", &["pem", "crt", "cer", "der"])
                                        .pick_file()
                                    {
                                        profile.tls.ca_cert_path = path.display().to_string();
                                    }
                                }
                            });

                            egui::CollapsingHeader::new("Optional Mutual TLS (Client Cert & Key)")
                                .default_open(false)
                                .show(ui, |ui| {
                                    ui.label("Client Certificate (.pem):");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut profile.tls.client_cert_path,
                                            )
                                            .desired_width(180.0),
                                        );
                                        if ui.small_button("📂").clicked() {
                                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                                profile.tls.client_cert_path =
                                                    p.display().to_string();
                                            }
                                        }
                                    });
                                    ui.label("Client Private Key (.pem):");
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::TextEdit::singleline(
                                                &mut profile.tls.client_key_path,
                                            )
                                            .desired_width(180.0),
                                        );
                                        if ui.small_button("📂").clicked() {
                                            if let Some(p) = rfd::FileDialog::new().pick_file() {
                                                profile.tls.client_key_path =
                                                    p.display().to_string();
                                            }
                                        }
                                    });
                                });
                        }
                    });

                ui.add_space(6.0);
                self.render_subscriptions_section(ui);

                let profile = self.current_profile_mut();
                ui.add_space(6.0);
                egui::CollapsingHeader::new(RichText::new("Last Will & Testament (LWT)").strong())
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.checkbox(&mut profile.last_will_enabled, "Enable Last Will");
                        if profile.last_will_enabled {
                            ui.label("Will Topic:");
                            ui.text_edit_singleline(&mut profile.last_will.topic);
                            ui.horizontal(|ui| {
                                egui::ComboBox::from_id_salt("lwt_qos")
                                    .selected_text(profile.last_will.qos.short_label())
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::AtMostOnce,
                                            "QoS 0",
                                        );
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::AtLeastOnce,
                                            "QoS 1",
                                        );
                                        ui.selectable_value(
                                            &mut profile.last_will.qos,
                                            QoS::ExactlyOnce,
                                            "QoS 2",
                                        );
                                    });
                                ui.checkbox(&mut profile.last_will.retain, "Retain");
                            });
                        }
                    });
            });
    }

    fn render_subscriptions_section(&mut self, ui: &mut Ui) {
        egui::CollapsingHeader::new(RichText::new("Subscriptions").strong())
            .default_open(true)
            .show(ui, |ui| {
                let mut remove_idx = None;
                let mut toggle_sub: Option<(String, QoS, bool)> = None;
                let is_connected = self.conn_state == ConnectionState::Connected;

                let subs = &mut self.current_profile_mut().subscriptions;
                for (i, sub) in subs.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut sub.enabled, "").changed() && is_connected {
                            toggle_sub = Some((sub.topic_filter.clone(), sub.qos, sub.enabled));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut sub.topic_filter).desired_width(130.0),
                        );
                        egui::ComboBox::from_id_salt(format!("sub_qos_{}", i))
                            .width(62.0)
                            .selected_text(sub.qos.short_label())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut sub.qos, QoS::AtMostOnce, "QoS 0");
                                ui.selectable_value(&mut sub.qos, QoS::AtLeastOnce, "QoS 1");
                                ui.selectable_value(&mut sub.qos, QoS::ExactlyOnce, "QoS 2");
                            });
                        if ui.small_button("✖").clicked() {
                            remove_idx = Some(i);
                        }
                    });
                }

                if let Some(i) = remove_idx {
                    let removed = self.current_profile_mut().subscriptions.remove(i);
                    if let Some(tx) = &self.cmd_tx {
                        let _ = tx.send(ClientCommand::Unsubscribe {
                            topic_filter: removed.topic_filter,
                        });
                    }
                }

                if let Some((filter, qos, en)) = toggle_sub {
                    if let Some(tx) = &self.cmd_tx {
                        if en {
                            let _ = tx.send(ClientCommand::Subscribe {
                                topic_filter: filter,
                                qos,
                            });
                        } else {
                            let _ = tx.send(ClientCommand::Unsubscribe {
                                topic_filter: filter,
                            });
                        }
                    }
                }

                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_sub_filter)
                            .hint_text("sensors/#")
                            .desired_width(130.0),
                    );
                    egui::ComboBox::from_id_salt("new_sub_qos")
                        .width(62.0)
                        .selected_text(self.new_sub_qos.short_label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.new_sub_qos, QoS::AtMostOnce, "QoS 0");
                            ui.selectable_value(&mut self.new_sub_qos, QoS::AtLeastOnce, "QoS 1");
                            ui.selectable_value(&mut self.new_sub_qos, QoS::ExactlyOnce, "QoS 2");
                        });
                    if ui.button("➕ Add").clicked() && !self.new_sub_filter.trim().is_empty() {
                        let filter = self.new_sub_filter.trim().to_string();
                        let qos = self.new_sub_qos;
                        self.current_profile_mut()
                            .subscriptions
                            .push(SubscriptionRule {
                                topic_filter: filter.clone(),
                                qos,
                                enabled: true,
                            });
                        if let Some(tx) = &self.cmd_tx {
                            let _ = tx.send(ClientCommand::Subscribe {
                                topic_filter: filter,
                                qos,
                            });
                        }
                        self.new_sub_filter.clear();
                    }
                });
            });
    }

    fn render_topic_tree_panel(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.heading("Topic Explorer");
            ui.separator();
            ui.label("🔍");
            ui.add(
                egui::TextEdit::singleline(&mut self.search_filter)
                    .hint_text("Filter topics or payload values...")
                    .desired_width(200.0),
            );
            if !self.search_filter.is_empty() && ui.small_button("✖").clicked() {
                self.search_filter.clear();
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("🗑 Clear").clicked() {
                    self.tree.clear();
                    self.selected_topic = None;
                }
                let pause_txt = if self.paused { "▶ Resume" } else { "⏸ Pause" };
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

    fn render_throughput_collapsing(&mut self, ui: &mut Ui) {
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

    fn render_root_topics_collapsing(&mut self, ui: &mut Ui) {
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

    fn render_right_panel(&mut self, ui: &mut Ui) {
        let available_h = ui.available_height();
        let graphs_reserved = if self.show_performance_panel {
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
            root_h + throughput_h
        } else {
            0.0_f32
        };

        let upper_h = (available_h - graphs_reserved).max(200.0);
        let inspector_h = if self.show_publish_drawer {
            (upper_h * 0.56).max(160.0)
        } else {
            (upper_h - 36.0).max(140.0)
        };

        ui.allocate_ui(egui::vec2(ui.available_width(), inspector_h), |ui| {
            self.render_inspector_section(ui);
        });

        ui.separator();

        ui.horizontal(|ui| {
            let toggle_icon = if self.show_publish_drawer { "-" } else { "+" };
            if ui
                .button(RichText::new(format!("{} Publish Message Drawer", toggle_icon)).strong())
                .clicked()
            {
                self.show_publish_drawer = !self.show_publish_drawer;
            }
            if let Some(sel) = &self.selected_topic {
                if ui.small_button("Use Selected Topic").clicked() {
                    self.publish_draft.topic = sel.clone();
                }
            }
        });

        if self.show_publish_drawer {
            let pub_h = (upper_h - inspector_h - 38.0).max(110.0);
            ui.allocate_ui(egui::vec2(ui.available_width(), pub_h), |ui| {
                ScrollArea::vertical()
                    .id_salt("publish_drawer_scroll")
                    .show(ui, |ui| {
                        self.render_publish_drawer(ui);
                    });
            });
        }

        if self.show_performance_panel {
            ui.separator();
            self.render_root_topics_collapsing(ui);
            ui.add_space(4.0);
            self.render_throughput_collapsing(ui);
        }
    }

    fn render_inspector_section(&mut self, ui: &mut Ui) {
        let Some(topic_path) = self.selected_topic.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(
                    RichText::new("Select any topic in the Topic Explorer to inspect payloads, MQTT v5 properties, history & diffs.")
                        .color(Color32::from_rgb(145, 155, 170)),
                );
            });
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
            ui.selectable_value(&mut self.inspector_tab, InspectorTab::Payload, "📄 Payload");
            ui.selectable_value(
                &mut self.inspector_tab,
                InspectorTab::Diff,
                format!("Diff View ({})", hist_len),
            );
            ui.selectable_value(
                &mut self.inspector_tab,
                InspectorTab::HistoryChart,
                "📈 Value Chart",
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

    fn render_publish_drawer(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label("Topic:");
            ui.add(
                egui::TextEdit::singleline(&mut self.publish_draft.topic)
                    .hint_text("sensors/temperature")
                    .desired_width(220.0),
            );

            egui::ComboBox::from_id_salt("pub_qos_combo")
                .width(70.0)
                .selected_text(self.publish_draft.qos.short_label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::AtMostOnce, "QoS 0");
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::AtLeastOnce, "QoS 1");
                    ui.selectable_value(&mut self.publish_draft.qos, QoS::ExactlyOnce, "QoS 2");
                });

            ui.checkbox(&mut self.publish_draft.retain, "Retain");

            let pub_btn = egui::Button::new(
                RichText::new("Publish")
                    .color(Color32::WHITE)
                    .strong(),
            )
            .fill(Color32::from_rgb(45, 115, 210));
            if ui.add(pub_btn).clicked() {
                self.execute_publish();
            }
        });

        ui.horizontal(|ui| {
            ui.label("Encode as:");
            egui::ComboBox::from_id_salt("pub_enc_combo")
                .selected_text(self.publish_draft.encoding.label())
                .show_ui(ui, |ui| {
                    for enc in [
                        PublishEncoding::Text,
                        PublishEncoding::Json,
                        PublishEncoding::Hex,
                        PublishEncoding::Base64,
                        PublishEncoding::MessagePackFromJson,
                        PublishEncoding::ProtobufFromJson,
                    ] {
                        ui.selectable_value(&mut self.publish_draft.encoding, enc, enc.label());
                    }
                });

            ui.checkbox(
                &mut self.publish_draft.show_v5_props,
                "MQTT v5 Properties",
            );
        });

        if self.publish_draft.show_v5_props {
            egui::Frame::group(ui.style())
                .fill(Color32::from_rgb(25, 30, 40))
                .show(ui, |ui| {
                    egui::Grid::new("pub_v5_props_grid")
                        .num_columns(4)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Content-Type:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.content_type)
                                    .desired_width(130.0),
                            );
                            ui.label("Expiry (s):");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.message_expiry)
                                    .hint_text("e.g. 3600")
                                    .desired_width(80.0),
                            );
                            ui.end_row();

                            ui.label("Response-Topic:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.publish_draft.response_topic)
                                    .desired_width(130.0),
                            );
                            ui.label("Correlation:");
                            ui.add(
                                egui::TextEdit::singleline(
                                    &mut self.publish_draft.correlation_data,
                                )
                                .desired_width(80.0),
                            );
                            ui.end_row();
                        });

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("User Properties:").size(11.5).strong());
                        if ui.small_button("➕ Add Property").clicked() {
                            self.publish_draft
                                .user_properties
                                .push((String::new(), String::new()));
                        }
                    });

                    let mut remove_prop = None;
                    for (idx, (k, v)) in self.publish_draft.user_properties.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(k)
                                    .hint_text("key")
                                    .desired_width(110.0),
                            );
                            ui.label("=");
                            ui.add(
                                egui::TextEdit::singleline(v)
                                    .hint_text("value")
                                    .desired_width(150.0),
                            );
                            if ui.small_button("✖").clicked() {
                                remove_prop = Some(idx);
                            }
                        });
                    }
                    if let Some(idx) = remove_prop {
                        self.publish_draft.user_properties.remove(idx);
                    }
                });
        }

        ui.add(
            egui::TextEdit::multiline(&mut self.publish_draft.payload_input)
                .font(egui::TextStyle::Monospace)
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
    }
}

impl MqttVersion {
    fn short_badge(&self) -> &'static str {
        match self {
            MqttVersion::V31 => "v3.1",
            MqttVersion::V311 => "v3.1.1",
            MqttVersion::V5 => "v5.0",
        }
    }
}

fn format_bytes(b: u64) -> String {
    if b < 1024 {
        format!("{} B", b)
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", b as f64 / (1024.0 * 1024.0))
    }
}

/// Paint a crisp anti-aliased status dot without relying on Unicode font glyphs.
fn draw_status_dot(ui: &mut Ui, radius: f32, color: Color32) {
    let side = (radius * 2.0 + 4.0).max(10.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), radius, color);
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

fn render_root_topics_chart(ui: &mut Ui, stats: &[RootTopicStat], total_msgs: u64) {
    if stats.is_empty() || total_msgs == 0 {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new("No topic data yet — click '🧪 Sample Data' or connect to a broker")
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
                        Pos2::new((row_rect.left() + fill_w).min(row_rect.right()), row_rect.bottom()),
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
                    let name_pos = Pos2::new(
                        name_left,
                        row_rect.center().y - name_galley.size().y * 0.5,
                    );
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
            painter.rect_filled(bar_rect, 2.0, Color32::from_rgba_unmultiplied(45, 195, 145, 175));
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

fn render_sparkline_chart(ui: &mut Ui, points: &[(f64, f64)]) {
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
