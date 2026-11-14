//! Async MQTT 3.1, 3.1.1, and 5.0 network client over Tokio + Rustls.
//! Supports TCP and TLS 1.2/1.3 (WebPKI + Native OS roots, Custom CA .pem/.crt/.der,
//! optional mTLS client certs, and "Skip certificate verification" mode).

use crate::config::{ConnectionProfile, TlsConfig};
use crate::protocol::{
    connack_error_description, decode_packet, encode_packet, ConnectParams, Mqtt5Properties,
    MqttPacket, MqttVersion, PublishPacket, QoS,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as TlsError, SignatureScheme};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Cursor};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsConnector;

#[derive(Debug, Clone)]
pub enum ClientCommand {
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: QoS,
        retain: bool,
        properties: Mqtt5Properties,
    },
    Subscribe {
        topic_filter: String,
        qos: QoS,
    },
    Unsubscribe {
        topic_filter: String,
    },
    Disconnect,
}

#[derive(Debug, Clone)]
pub enum ClientEvent {
    Connecting {
        host: String,
        port: u16,
        tls: bool,
        version: MqttVersion,
    },
    Connected {
        session_present: bool,
        assigned_client_id: Option<String>,
        server_keep_alive: Option<u16>,
    },
    MessageReceived {
        topic: String,
        payload: Vec<u8>,
        qos: QoS,
        retain: bool,
        properties: Mqtt5Properties,
    },
    MessagePublished {
        topic: String,
        bytes: usize,
    },
    Subscribed {
        topics: Vec<String>,
    },
    Unsubscribed {
        topics: Vec<String>,
    },
    Log(String),
    Error(String),
    Disconnected(String),
}

enum MqttStream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

impl AsyncRead for MqttStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MqttStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MqttStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MqttStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MqttStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MqttStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MqttStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MqttStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MqttStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MqttStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

#[derive(Debug)]
struct NoCertificateVerification(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, TlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, TlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn build_tls_connector(tls_cfg: &TlsConfig) -> Result<TlsConnector, String> {
    let provider = Arc::new(rustls_rustcrypto::provider());

    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS protocol config error: {}", e))?;

    let client_auth = load_client_auth(tls_cfg)?;

    let config = if tls_cfg.accept_invalid_certs {
        let verifier = Arc::new(NoCertificateVerification(provider));
        let danger_builder = builder.dangerous().with_custom_certificate_verifier(verifier);
        match client_auth {
            Some((certs, key)) => danger_builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("Invalid client cert/key: {}", e))?,
            None => danger_builder.with_no_client_auth(),
        }
    } else {
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        // Also attempt loading OS native certificates
        let native = rustls_native_certs::load_native_certs();
        for cert in native.certs {
            let _ = root_store.add(cert);
        }

        // Load custom CA certificate if specified
        let ca_path = tls_cfg.ca_cert_path.trim();
        if !ca_path.is_empty() {
            let custom_certs = load_certs_from_file(ca_path)?;
            if custom_certs.is_empty() {
                return Err(format!("No certificates found in CA file: {}", ca_path));
            }
            for cert in custom_certs {
                root_store
                    .add(cert)
                    .map_err(|e| format!("Failed to add custom CA cert: {}", e))?;
            }
        }

        let std_builder = builder.with_root_certificates(root_store);
        match client_auth {
            Some((certs, key)) => std_builder
                .with_client_auth_cert(certs, key)
                .map_err(|e| format!("Invalid client cert/key: {}", e))?,
            None => std_builder.with_no_client_auth(),
        }
    };

    Ok(TlsConnector::from(Arc::new(config)))
}

fn load_certs_from_file(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("Failed to read '{}': {}", path, e))?;
    let mut reader = BufReader::new(Cursor::new(&raw));
    let mut pem_certs = Vec::new();
    for item in rustls_pemfile::certs(&mut reader) {
        match item {
            Ok(c) => pem_certs.push(c),
            Err(e) => return Err(format!("Failed to parse PEM cert in '{}': {}", path, e)),
        }
    }
    if !pem_certs.is_empty() {
        return Ok(pem_certs);
    }
    // Treat as raw DER certificate
    Ok(vec![CertificateDer::from(raw)])
}

fn load_client_auth(
    tls_cfg: &TlsConfig,
) -> Result<Option<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)>, String> {
    let cert_path = tls_cfg.client_cert_path.trim();
    let key_path = tls_cfg.client_key_path.trim();
    if cert_path.is_empty() || key_path.is_empty() {
        return Ok(None);
    }
    let certs = load_certs_from_file(cert_path)?;
    let key_file =
        File::open(key_path).map_err(|e| format!("Failed to open key '{}': {}", key_path, e))?;
    let mut reader = BufReader::new(key_file);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("Failed to parse private key '{}': {}", key_path, e))?
        .ok_or_else(|| format!("No private key found in '{}'", key_path))?;
    Ok(Some((certs, key)))
}

pub fn spawn_mqtt_session(
    profile: ConnectionProfile,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
) -> mpsc::UnboundedSender<ClientCommand> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");

        rt.block_on(async move {
            if let Err(err) = run_mqtt_loop(profile, cmd_rx, event_tx.clone()).await {
                let _ = event_tx.send(ClientEvent::Error(err.clone()));
                let _ = event_tx.send(ClientEvent::Disconnected(err));
            }
        });
    });

    cmd_tx
}

async fn run_mqtt_loop(
    profile: ConnectionProfile,
    mut cmd_rx: mpsc::UnboundedReceiver<ClientCommand>,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
) -> Result<(), String> {
    let host = profile.host.trim().to_string();
    let port = profile.port;
    let version = profile.version;

    let _ = event_tx.send(ClientEvent::Connecting {
        host: host.clone(),
        port,
        tls: profile.tls.enabled,
        version,
    });

    let addr = format!("{}:{}", host, port);
    let tcp = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&addr))
        .await
        .map_err(|_| format!("Connection timed out connecting to {}", addr))?
        .map_err(|e| format!("TCP connection to {} failed: {}", addr, e))?;
    let _ = tcp.set_nodelay(true);

    let mut stream = if profile.tls.enabled {
        let connector = build_tls_connector(&profile.tls)?;
        let server_name = ServerName::try_from(host.clone())
            .map_err(|_| format!("Invalid TLS server hostname: '{}'", host))?;
        let tls_stream = tokio::time::timeout(
            Duration::from_secs(10),
            connector.connect(server_name, tcp),
        )
        .await
        .map_err(|_| "TLS handshake timed out".to_string())?
        .map_err(|e| format!("TLS handshake failed: {}", e))?;
        MqttStream::Tls(Box::new(tls_stream))
    } else {
        MqttStream::Plain(tcp)
    };

    // Build and send CONNECT packet
    let mut connect_props = Mqtt5Properties::default();
    connect_props.session_expiry_interval = profile.session_expiry_interval;
    connect_props.receive_maximum = profile.receive_maximum;

    let connect_params = ConnectParams {
        version,
        client_id: profile.client_id.clone(),
        clean_session: profile.clean_session,
        keep_alive_secs: profile.keep_alive_secs,
        username: if profile.username.trim().is_empty() {
            None
        } else {
            Some(profile.username.clone())
        },
        password: if profile.password.is_empty() {
            None
        } else {
            Some(profile.password.clone())
        },
        last_will: if profile.last_will_enabled {
            Some(profile.last_will.clone())
        } else {
            None
        },
        properties: connect_props,
    };

    let connect_bytes = encode_packet(&MqttPacket::Connect(connect_params), version);
    stream
        .write_all(&connect_bytes)
        .await
        .map_err(|e| format!("Failed to send CONNECT packet: {}", e))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("Failed to flush CONNECT: {}", e))?;

    // Wait for CONNACK
    let connack = tokio::time::timeout(Duration::from_secs(10), read_next_packet(&mut stream, version))
        .await
        .map_err(|_| "Timed out waiting for CONNACK from broker".to_string())?
        .map_err(|e| format!("Failed reading CONNACK: {}", e))?;

    let mut keep_alive_secs = profile.keep_alive_secs.max(5);
    match connack {
        MqttPacket::ConnAck {
            session_present,
            return_code,
            properties,
        } => {
            if return_code != 0 {
                let reason = connack_error_description(return_code, version);
                let detail = properties
                    .reason_string
                    .map(|s| format!(" ({})", s))
                    .unwrap_or_default();
                return Err(format!("Broker rejected connection: {}{}", reason, detail));
            }
            if let Some(ska) = properties.server_keep_alive {
                keep_alive_secs = ska.max(5);
            }
            let _ = event_tx.send(ClientEvent::Connected {
                session_present,
                assigned_client_id: properties.assigned_client_identifier,
                server_keep_alive: properties.server_keep_alive,
            });
        }
        other => {
            return Err(format!("Expected CONNACK, received {:?}", other));
        }
    }

    let mut next_packet_id: u16 = 1;
    let mut topic_aliases: HashMap<u16, String> = HashMap::new();

    // Send initial subscriptions
    let active_subs: Vec<(String, QoS)> = profile
        .subscriptions
        .iter()
        .filter(|s| s.enabled && !s.topic_filter.trim().is_empty())
        .map(|s| (s.topic_filter.trim().to_string(), s.qos))
        .collect();

    if !active_subs.is_empty() {
        let pid = alloc_packet_id(&mut next_packet_id);
        let sub_pkt = MqttPacket::Subscribe {
            packet_id: pid,
            topics: active_subs.clone(),
            properties: Mqtt5Properties::default(),
        };
        let raw = encode_packet(&sub_pkt, version);
        stream
            .write_all(&raw)
            .await
            .map_err(|e| format!("Failed to send initial SUBSCRIBE: {}", e))?;
        let _ = event_tx.send(ClientEvent::Subscribed {
            topics: active_subs.into_iter().map(|(t, _)| t).collect(),
        });
    }

    let mut ping_interval = tokio::time::interval(Duration::from_secs(keep_alive_secs as u64));
    ping_interval.tick().await; // skip immediate tick

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                let raw = encode_packet(&MqttPacket::PingReq, version);
                if let Err(e) = stream.write_all(&raw).await {
                    return Err(format!("Failed to send PINGREQ: {}", e));
                }
            }

            cmd_opt = cmd_rx.recv() => {
                match cmd_opt {
                    Some(ClientCommand::Publish { topic, payload, qos, retain, properties }) => {
                        let packet_id = if qos != QoS::AtMostOnce {
                            Some(alloc_packet_id(&mut next_packet_id))
                        } else {
                            None
                        };
                        let payload_len = payload.len();
                        let pkt = MqttPacket::Publish(PublishPacket {
                            dup: false,
                            qos,
                            retain,
                            topic: topic.clone(),
                            packet_id,
                            payload,
                            properties,
                        });
                        let raw = encode_packet(&pkt, version);
                        stream.write_all(&raw).await.map_err(|e| format!("Publish write failed: {}", e))?;
                        let _ = event_tx.send(ClientEvent::MessagePublished {
                            topic,
                            bytes: payload_len,
                        });
                    }
                    Some(ClientCommand::Subscribe { topic_filter, qos }) => {
                        let pid = alloc_packet_id(&mut next_packet_id);
                        let pkt = MqttPacket::Subscribe {
                            packet_id: pid,
                            topics: vec![(topic_filter.clone(), qos)],
                            properties: Mqtt5Properties::default(),
                        };
                        let raw = encode_packet(&pkt, version);
                        stream.write_all(&raw).await.map_err(|e| format!("Subscribe write failed: {}", e))?;
                        let _ = event_tx.send(ClientEvent::Subscribed {
                            topics: vec![topic_filter],
                        });
                    }
                    Some(ClientCommand::Unsubscribe { topic_filter }) => {
                        let pid = alloc_packet_id(&mut next_packet_id);
                        let pkt = MqttPacket::Unsubscribe {
                            packet_id: pid,
                            topics: vec![topic_filter.clone()],
                        };
                        let raw = encode_packet(&pkt, version);
                        stream.write_all(&raw).await.map_err(|e| format!("Unsubscribe write failed: {}", e))?;
                        let _ = event_tx.send(ClientEvent::Unsubscribed {
                            topics: vec![topic_filter],
                        });
                    }
                    Some(ClientCommand::Disconnect) | None => {
                        let disc = MqttPacket::Disconnect {
                            reason_code: 0,
                            properties: Mqtt5Properties::default(),
                        };
                        let raw = encode_packet(&disc, version);
                        let _ = stream.write_all(&raw).await;
                        let _ = stream.shutdown().await;
                        let _ = event_tx.send(ClientEvent::Disconnected("Disconnected by user".to_string()));
                        return Ok(());
                    }
                }
            }

            pkt_res = read_next_packet(&mut stream, version) => {
                let pkt = pkt_res.map_err(|e| format!("Connection closed or read error: {}", e))?;
                match pkt {
                    MqttPacket::Publish(mut pub_pkt) => {
                        // Resolve MQTT v5 Topic Alias if used
                        if let Some(alias) = pub_pkt.properties.topic_alias {
                            if !pub_pkt.topic.is_empty() {
                                topic_aliases.insert(alias, pub_pkt.topic.clone());
                            } else if let Some(existing) = topic_aliases.get(&alias) {
                                pub_pkt.topic = existing.clone();
                            }
                        }

                        // Handle QoS acknowledgements
                        match pub_pkt.qos {
                            QoS::AtMostOnce => {}
                            QoS::AtLeastOnce => {
                                if let Some(pid) = pub_pkt.packet_id {
                                    let ack = MqttPacket::PubAck {
                                        packet_id: pid,
                                        reason_code: 0,
                                        properties: Mqtt5Properties::default(),
                                    };
                                    let raw = encode_packet(&ack, version);
                                    let _ = stream.write_all(&raw).await;
                                }
                            }
                            QoS::ExactlyOnce => {
                                if let Some(pid) = pub_pkt.packet_id {
                                    let rec = MqttPacket::PubRec {
                                        packet_id: pid,
                                        reason_code: 0,
                                    };
                                    let raw = encode_packet(&rec, version);
                                    let _ = stream.write_all(&raw).await;
                                }
                            }
                        }

                        let _ = event_tx.send(ClientEvent::MessageReceived {
                            topic: pub_pkt.topic,
                            payload: pub_pkt.payload,
                            qos: pub_pkt.qos,
                            retain: pub_pkt.retain,
                            properties: pub_pkt.properties,
                        });
                    }

                    MqttPacket::PubRec { packet_id, .. } => {
                        let rel = MqttPacket::PubRel {
                            packet_id,
                            reason_code: 0,
                        };
                        let raw = encode_packet(&rel, version);
                        let _ = stream.write_all(&raw).await;
                    }

                    MqttPacket::PubRel { packet_id, .. } => {
                        let comp = MqttPacket::PubComp {
                            packet_id,
                            reason_code: 0,
                        };
                        let raw = encode_packet(&comp, version);
                        let _ = stream.write_all(&raw).await;
                    }

                    MqttPacket::SubAck {
                        packet_id,
                        return_codes,
                        properties,
                    } => {
                        let detail = properties
                            .reason_string
                            .map(|s| format!(" ({})", s))
                            .unwrap_or_default();
                        let _ = event_tx.send(ClientEvent::Log(format!(
                            "SUBACK (id={}, codes={:?}){}",
                            packet_id, return_codes, detail
                        )));
                    }

                    MqttPacket::UnsubAck { packet_id } => {
                        let _ = event_tx.send(ClientEvent::Log(format!(
                            "UNSUBACK (id={})",
                            packet_id
                        )));
                    }

                    MqttPacket::PingReq => {
                        let raw = encode_packet(&MqttPacket::PingResp, version);
                        let _ = stream.write_all(&raw).await;
                    }

                    MqttPacket::Disconnect { reason_code, properties } => {
                        let reason = properties
                            .reason_string
                            .unwrap_or_else(|| format!("Broker sent DISCONNECT (code 0x{:02X})", reason_code));
                        let _ = event_tx.send(ClientEvent::Disconnected(reason));
                        return Ok(());
                    }

                    _ => {}
                }
            }
        }
    }
}

fn alloc_packet_id(counter: &mut u16) -> u16 {
    let id = *counter;
    *counter = if *counter == u16::MAX { 1 } else { *counter + 1 };
    id
}

async fn read_next_packet(
    stream: &mut MqttStream,
    version: MqttVersion,
) -> io::Result<MqttPacket> {
    let mut first = [0u8; 1];
    stream.read_exact(&mut first).await?;

    // Read Remaining Length variable byte integer
    let mut multiplier: usize = 1;
    let mut rem_len: usize = 0;
    loop {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b).await?;
        let encoded_byte = b[0];
        rem_len += ((encoded_byte & 127) as usize) * multiplier;
        if (encoded_byte & 128) == 0 {
            break;
        }
        multiplier *= 128;
        if multiplier > 128 * 128 * 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Malformed MQTT Remaining Length",
            ));
        }
    }

    if rem_len > 64 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "MQTT packet exceeds 64 MiB safety limit",
        ));
    }

    let mut body = vec![0u8; rem_len];
    if rem_len > 0 {
        stream.read_exact(&mut body).await?;
    }

    decode_packet(first[0], &body, version)
}
