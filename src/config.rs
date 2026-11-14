//! Saved connection profiles and portable configuration management (TOML & JSON).

use crate::protocol::{LastWill, MqttVersion, QoS};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRule {
    pub topic_filter: String,
    pub qos: QoS,
    pub enabled: bool,
}

impl Default for SubscriptionRule {
    fn default() -> Self {
        Self {
            topic_filter: "#".to_string(),
            qos: QoS::AtMostOnce,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TlsConfig {
    pub enabled: bool,
    /// Optional path to a custom CA certificate (.pem / .crt / .der)
    pub ca_cert_path: String,
    /// Skip server certificate verification (useful for self-signed certs / local testing)
    pub accept_invalid_certs: bool,
    /// Optional client certificate path (.pem) for mutual TLS
    pub client_cert_path: String,
    /// Optional client private key path (.pem) for mutual TLS
    pub client_key_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub version: MqttVersion,
    pub client_id: String,
    pub clean_session: bool,
    pub keep_alive_secs: u16,
    pub username: String,
    pub password: String,
    pub tls: TlsConfig,
    pub subscriptions: Vec<SubscriptionRule>,
    pub session_expiry_interval: Option<u32>,
    pub receive_maximum: Option<u16>,
    pub last_will_enabled: bool,
    pub last_will: LastWill,
}

impl Default for ConnectionProfile {
    fn default() -> Self {
        Self {
            id: uuid_short(),
            name: "Local Broker".to_string(),
            host: "localhost".to_string(),
            port: 1883,
            version: MqttVersion::V5,
            client_id: format!("mqtt-explorer-rs-{}", uuid_short()),
            clean_session: true,
            keep_alive_secs: 30,
            username: String::new(),
            password: String::new(),
            tls: TlsConfig::default(),
            subscriptions: vec![
                SubscriptionRule {
                    topic_filter: "#".to_string(),
                    qos: QoS::AtMostOnce,
                    enabled: true,
                },
                SubscriptionRule {
                    topic_filter: "$SYS/#".to_string(),
                    qos: QoS::AtMostOnce,
                    enabled: false,
                },
            ],
            session_expiry_interval: None,
            receive_maximum: None,
            last_will_enabled: false,
            last_will: LastWill {
                topic: "clients/status".to_string(),
                payload: b"offline".to_vec(),
                qos: QoS::AtMostOnce,
                retain: false,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub selected_profile_idx: usize,
    pub max_history_per_topic: usize,
    pub profiles: Vec<ConnectionProfile>,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mosquitto_tcp = ConnectionProfile {
            id: "mosquitto-tcp".to_string(),
            name: "Mosquitto Public (TCP)".to_string(),
            host: "test.mosquitto.org".to_string(),
            port: 1883,
            version: MqttVersion::V5,
            client_id: format!("mqtt-rs-{}", uuid_short()),
            ..Default::default()
        };

        let mosquitto_tls = ConnectionProfile {
            id: "mosquitto-tls".to_string(),
            name: "Mosquitto Public (TLS)".to_string(),
            host: "test.mosquitto.org".to_string(),
            port: 8883,
            version: MqttVersion::V5,
            client_id: format!("mqtt-rs-tls-{}", uuid_short()),
            tls: TlsConfig {
                enabled: true,
                ca_cert_path: String::new(),
                accept_invalid_certs: true, // test.mosquitto.org uses its own test CA on 8883
                client_cert_path: String::new(),
                client_key_path: String::new(),
            },
            ..Default::default()
        };

        let emqx_tls = ConnectionProfile {
            id: "emqx-tls".to_string(),
            name: "EMQX Public (TLS 8883)".to_string(),
            host: "broker.emqx.io".to_string(),
            port: 8883,
            version: MqttVersion::V5,
            client_id: format!("mqtt-rs-emqx-{}", uuid_short()),
            tls: TlsConfig {
                enabled: true,
                ca_cert_path: String::new(),
                accept_invalid_certs: false,
                client_cert_path: String::new(),
                client_key_path: String::new(),
            },
            ..Default::default()
        };

        let local = ConnectionProfile::default();

        Self {
            selected_profile_idx: 0,
            max_history_per_topic: 50,
            profiles: vec![local, emqx_tls, mosquitto_tcp, mosquitto_tls],
        }
    }
}

impl AppConfig {
    pub fn default_config_path() -> PathBuf {
        PathBuf::from("mqtt_profiles.toml")
    }

    pub fn load_or_default() -> Self {
        let path = Self::default_config_path();
        if path.exists() {
            if let Ok(cfg) = Self::load_from_file(&path) {
                if !cfg.profiles.is_empty() {
                    return cfg;
                }
            }
        }
        let cfg = Self::default();
        let _ = cfg.save_to_file(&path);
        cfg
    }

    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("toml")
            .to_ascii_lowercase();

        if ext == "json" {
            serde_json::from_str(&content)
                .map_err(|e| format!("Failed to parse JSON config: {}", e))
        } else {
            toml::from_str(&content).map_err(|e| format!("Failed to parse TOML config: {}", e))
        }
    }

    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("toml")
            .to_ascii_lowercase();

        let serialized = if ext == "json" {
            serde_json::to_string_pretty(self)
                .map_err(|e| format!("Failed to serialize JSON: {}", e))?
        } else {
            toml::to_string_pretty(self).map_err(|e| format!("Failed to serialize TOML: {}", e))?
        };

        fs::write(path, serialized)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))
    }
}

pub fn uuid_short() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ (d.as_secs() as u32))
        .unwrap_or(123456);
    format!("{:06x}", nanos & 0xFFFFFF)
}
