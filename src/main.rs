#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod client;
mod config;
mod icon;
#[cfg(target_os = "linux")]
mod linux_integration;
mod payload;
mod protocol;
mod tree;
mod ui;

use client::{spawn_mqtt_session, ClientCommand, ClientEvent};
use config::{AppConfig, ConnectionProfile};
use eframe::egui::{self, Color32};
use payload::{encode_publish_payload, PayloadFormat, PublishEncoding};
use protocol::{Mqtt5Properties, MqttVersion, QoS};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::mpsc;
use tree::TopicTree;

fn main() -> eframe::Result<()> {
    #[cfg(target_os = "linux")]
    {
        // Allow explicit registration without opening a window:
        // `mqtt-client --install-desktop-entry`
        if std::env::args().any(|arg| arg == "--install-desktop-entry") {
            match linux_integration::ensure_desktop_entry() {
                Some(path) => {
                    println!("Desktop entry installed/updated: {}", path.display());
                    println!("App icon installed into the hicolor icon theme under $XDG_DATA_HOME/icons.");
                }
                None => {
                    eprintln!("Could not install desktop entry (no writable XDG data home found).")
                }
            }
            return Ok(());
        }
        let _ = linux_integration::ensure_desktop_entry();
    }

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

pub(crate) struct MqttApp {
    config: AppConfig,
    show_profile_sidebar: bool,
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
        visuals.indent_has_left_vline = false;
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

    pub(crate) fn current_profile(&self) -> &ConnectionProfile {
        let idx = self
            .config
            .selected_profile_idx
            .min(self.config.profiles.len().saturating_sub(1));
        &self.config.profiles[idx]
    }

    pub(crate) fn current_profile_mut(&mut self) -> &mut ConnectionProfile {
        if self.config.profiles.is_empty() {
            self.config.profiles.push(ConnectionProfile::default());
        }
        let idx = self
            .config
            .selected_profile_idx
            .min(self.config.profiles.len() - 1);
        &mut self.config.profiles[idx]
    }

    pub(crate) fn connect_selected_profile(&mut self) {
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

    pub(crate) fn disconnect(&mut self) {
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

    pub(crate) fn inject_sample_traffic(&mut self) {
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

    pub(crate) fn execute_publish(&mut self) {
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
                .size_range(260.0..=520.0)
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
