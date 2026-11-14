//! Complete MQTT 3.1 (MQIsdp), 3.1.1, and 5.0 wire protocol codec and types.
//!
//! Designed to be 100% pure Rust with zero external C dependencies, supporting:
//! - MQTT 3.1 (Protocol Name "MQIsdp", level 3)
//! - MQTT 3.1.1 (Protocol Name "MQTT", level 4)
//! - MQTT 5.0 (Protocol Name "MQTT", level 5) with full v5 properties support

use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, Cursor, Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MqttVersion {
    V31,
    V311,
    V5,
}

impl MqttVersion {
    pub fn protocol_name(&self) -> &'static str {
        match self {
            MqttVersion::V31 => "MQIsdp",
            MqttVersion::V311 | MqttVersion::V5 => "MQTT",
        }
    }

    pub fn protocol_level(&self) -> u8 {
        match self {
            MqttVersion::V31 => 3,
            MqttVersion::V311 => 4,
            MqttVersion::V5 => 5,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            MqttVersion::V31 => "MQTT 3.1 (v3)",
            MqttVersion::V311 => "MQTT 3.1.1 (v4)",
            MqttVersion::V5 => "MQTT 5.0 (v5)",
        }
    }
}

impl fmt::Display for MqttVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QoS {
    AtMostOnce = 0,
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

impl QoS {
    pub fn from_u8(val: u8) -> Result<Self, String> {
        match val {
            0 => Ok(QoS::AtMostOnce),
            1 => Ok(QoS::AtLeastOnce),
            2 => Ok(QoS::ExactlyOnce),
            other => Err(format!("Invalid QoS value: {}", other)),
        }
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn short_label(&self) -> &'static str {
        match self {
            QoS::AtMostOnce => "QoS 0",
            QoS::AtLeastOnce => "QoS 1",
            QoS::ExactlyOnce => "QoS 2",
        }
    }
}

/// MQTT 5.0 Properties supported across packets (PUBLISH, CONNECT, CONNACK, DISCONNECT, etc.)
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mqtt5Properties {
    pub payload_format_indicator: Option<u8>,
    pub message_expiry_interval: Option<u32>,
    pub content_type: Option<String>,
    pub response_topic: Option<String>,
    pub correlation_data: Option<Vec<u8>>,
    pub subscription_identifiers: Vec<u32>,
    pub session_expiry_interval: Option<u32>,
    pub assigned_client_identifier: Option<String>,
    pub server_keep_alive: Option<u16>,
    pub authentication_method: Option<String>,
    pub authentication_data: Option<Vec<u8>>,
    pub request_problem_information: Option<u8>,
    pub will_delay_interval: Option<u32>,
    pub request_response_information: Option<u8>,
    pub response_information: Option<String>,
    pub server_reference: Option<String>,
    pub reason_string: Option<String>,
    pub receive_maximum: Option<u16>,
    pub topic_alias_maximum: Option<u16>,
    pub topic_alias: Option<u16>,
    pub maximum_qos: Option<u8>,
    pub retain_available: Option<u8>,
    pub user_properties: Vec<(String, String)>,
    pub maximum_packet_size: Option<u32>,
}

impl Mqtt5Properties {
    pub fn is_empty(&self) -> bool {
        self.payload_format_indicator.is_none()
            && self.message_expiry_interval.is_none()
            && self.content_type.is_none()
            && self.response_topic.is_none()
            && self.correlation_data.is_none()
            && self.subscription_identifiers.is_empty()
            && self.session_expiry_interval.is_none()
            && self.assigned_client_identifier.is_none()
            && self.server_keep_alive.is_none()
            && self.authentication_method.is_none()
            && self.authentication_data.is_none()
            && self.reason_string.is_none()
            && self.topic_alias.is_none()
            && self.user_properties.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        if let Some(v) = self.payload_format_indicator {
            buf.push(0x01);
            buf.push(v);
        }
        if let Some(v) = self.message_expiry_interval {
            buf.push(0x02);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(ref s) = self.content_type {
            buf.push(0x03);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(ref s) = self.response_topic {
            buf.push(0x08);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(ref b) = self.correlation_data {
            buf.push(0x09);
            write_mqtt_bytes(&mut buf, b);
        }
        for &sub_id in &self.subscription_identifiers {
            buf.push(0x0B);
            write_var_int(&mut buf, sub_id as usize);
        }
        if let Some(v) = self.session_expiry_interval {
            buf.push(0x11);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(ref s) = self.assigned_client_identifier {
            buf.push(0x12);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(v) = self.server_keep_alive {
            buf.push(0x13);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(ref s) = self.authentication_method {
            buf.push(0x15);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(ref b) = self.authentication_data {
            buf.push(0x16);
            write_mqtt_bytes(&mut buf, b);
        }
        if let Some(v) = self.request_problem_information {
            buf.push(0x17);
            buf.push(v);
        }
        if let Some(v) = self.will_delay_interval {
            buf.push(0x18);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(v) = self.request_response_information {
            buf.push(0x19);
            buf.push(v);
        }
        if let Some(ref s) = self.response_information {
            buf.push(0x1A);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(ref s) = self.server_reference {
            buf.push(0x1C);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(ref s) = self.reason_string {
            buf.push(0x1F);
            write_mqtt_string(&mut buf, s);
        }
        if let Some(v) = self.receive_maximum {
            buf.push(0x21);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(v) = self.topic_alias_maximum {
            buf.push(0x22);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        if let Some(v) = self.topic_alias {
            buf.push(0x23);
            buf.extend_from_slice(&v.to_be_bytes());
        }
        for (k, v) in &self.user_properties {
            if !k.is_empty() || !v.is_empty() {
                buf.push(0x26);
                write_mqtt_string(&mut buf, k);
                write_mqtt_string(&mut buf, v);
            }
        }
        if let Some(v) = self.maximum_packet_size {
            buf.push(0x27);
            buf.extend_from_slice(&v.to_be_bytes());
        }

        let mut out = Vec::with_capacity(buf.len() + 4);
        write_var_int(&mut out, buf.len());
        out.extend_from_slice(&buf);
        out
    }

    pub fn decode(cur: &mut Cursor<&[u8]>) -> io::Result<Self> {
        let prop_len = read_var_int(cur)?;
        let start_pos = cur.position() as usize;
        let end_pos = start_pos + prop_len;
        let slice_len = cur.get_ref().len();
        if end_pos > slice_len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "MQTT v5 properties length exceeds packet bounds",
            ));
        }

        let mut props = Mqtt5Properties::default();
        while (cur.position() as usize) < end_pos {
            let prop_id = read_u8(cur)?;
            match prop_id {
                0x01 => props.payload_format_indicator = Some(read_u8(cur)?),
                0x02 => props.message_expiry_interval = Some(read_u32(cur)?),
                0x03 => props.content_type = Some(read_mqtt_string(cur)?),
                0x08 => props.response_topic = Some(read_mqtt_string(cur)?),
                0x09 => props.correlation_data = Some(read_mqtt_bytes(cur)?),
                0x0B => {
                    let id = read_var_int(cur)? as u32;
                    props.subscription_identifiers.push(id);
                }
                0x11 => props.session_expiry_interval = Some(read_u32(cur)?),
                0x12 => props.assigned_client_identifier = Some(read_mqtt_string(cur)?),
                0x13 => props.server_keep_alive = Some(read_u16(cur)?),
                0x15 => props.authentication_method = Some(read_mqtt_string(cur)?),
                0x16 => props.authentication_data = Some(read_mqtt_bytes(cur)?),
                0x17 => props.request_problem_information = Some(read_u8(cur)?),
                0x18 => props.will_delay_interval = Some(read_u32(cur)?),
                0x19 => props.request_response_information = Some(read_u8(cur)?),
                0x1A => props.response_information = Some(read_mqtt_string(cur)?),
                0x1C => props.server_reference = Some(read_mqtt_string(cur)?),
                0x1F => props.reason_string = Some(read_mqtt_string(cur)?),
                0x21 => props.receive_maximum = Some(read_u16(cur)?),
                0x22 => props.topic_alias_maximum = Some(read_u16(cur)?),
                0x23 => props.topic_alias = Some(read_u16(cur)?),
                0x24 => props.maximum_qos = Some(read_u8(cur)?),
                0x25 => props.retain_available = Some(read_u8(cur)?),
                0x26 => {
                    let k = read_mqtt_string(cur)?;
                    let v = read_mqtt_string(cur)?;
                    props.user_properties.push((k, v));
                }
                0x27 => props.maximum_packet_size = Some(read_u32(cur)?),
                0x28 | 0x29 | 0x2A => {
                    let _ = read_u8(cur)?;
                }
                unknown => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Unknown MQTT v5 property identifier: 0x{:02X}", unknown),
                    ));
                }
            }
        }
        Ok(props)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastWill {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: QoS,
    pub retain: bool,
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub version: MqttVersion,
    pub client_id: String,
    pub clean_session: bool,
    pub keep_alive_secs: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub last_will: Option<LastWill>,
    pub properties: Mqtt5Properties,
}

#[derive(Debug, Clone)]
pub struct PublishPacket {
    pub dup: bool,
    pub qos: QoS,
    pub retain: bool,
    pub topic: String,
    pub packet_id: Option<u16>,
    pub payload: Vec<u8>,
    pub properties: Mqtt5Properties,
}

#[derive(Debug, Clone)]
pub enum MqttPacket {
    Connect(ConnectParams),
    ConnAck {
        session_present: bool,
        return_code: u8,
        properties: Mqtt5Properties,
    },
    Publish(PublishPacket),
    PubAck {
        packet_id: u16,
        reason_code: u8,
        properties: Mqtt5Properties,
    },
    PubRec {
        packet_id: u16,
        reason_code: u8,
    },
    PubRel {
        packet_id: u16,
        reason_code: u8,
    },
    PubComp {
        packet_id: u16,
        reason_code: u8,
    },
    Subscribe {
        packet_id: u16,
        topics: Vec<(String, QoS)>,
        properties: Mqtt5Properties,
    },
    SubAck {
        packet_id: u16,
        return_codes: Vec<u8>,
        properties: Mqtt5Properties,
    },
    Unsubscribe {
        packet_id: u16,
        topics: Vec<String>,
    },
    UnsubAck {
        packet_id: u16,
    },
    PingReq,
    PingResp,
    Disconnect {
        reason_code: u8,
        properties: Mqtt5Properties,
    },
}

pub fn encode_packet(packet: &MqttPacket, version: MqttVersion) -> Vec<u8> {
    match packet {
        MqttPacket::Connect(params) => {
            let mut body = Vec::new();
            write_mqtt_string(&mut body, params.version.protocol_name());
            body.push(params.version.protocol_level());

            let mut flags: u8 = 0;
            if params.clean_session {
                flags |= 0x02;
            }
            if let Some(ref will) = params.last_will {
                flags |= 0x04;
                flags |= (will.qos.as_u8() & 0x03) << 3;
                if will.retain {
                    flags |= 0x20;
                }
            }
            if params.password.is_some() {
                flags |= 0x40;
            }
            if params.username.is_some() {
                flags |= 0x80;
            }
            body.push(flags);
            body.extend_from_slice(&params.keep_alive_secs.to_be_bytes());

            if params.version == MqttVersion::V5 {
                body.extend_from_slice(&params.properties.encode());
            }

            // Payload
            write_mqtt_string(&mut body, &params.client_id);

            if let Some(ref will) = params.last_will {
                if params.version == MqttVersion::V5 {
                    // Empty Will Properties
                    body.push(0x00);
                }
                write_mqtt_string(&mut body, &will.topic);
                write_mqtt_bytes(&mut body, &will.payload);
            }

            if let Some(ref user) = params.username {
                write_mqtt_string(&mut body, user);
            }
            if let Some(ref pass) = params.password {
                write_mqtt_string(&mut body, pass);
            }

            build_fixed_header(0x10, &body)
        }

        MqttPacket::Publish(pub_pkt) => {
            let mut header_byte = 0x30;
            if pub_pkt.dup {
                header_byte |= 0x08;
            }
            header_byte |= (pub_pkt.qos.as_u8() & 0x03) << 1;
            if pub_pkt.retain {
                header_byte |= 0x01;
            }

            let mut body = Vec::with_capacity(pub_pkt.topic.len() + pub_pkt.payload.len() + 16);
            write_mqtt_string(&mut body, &pub_pkt.topic);
            if pub_pkt.qos != QoS::AtMostOnce {
                let pid = pub_pkt.packet_id.unwrap_or(1);
                body.extend_from_slice(&pid.to_be_bytes());
            }
            if version == MqttVersion::V5 {
                body.extend_from_slice(&pub_pkt.properties.encode());
            }
            body.extend_from_slice(&pub_pkt.payload);

            build_fixed_header(header_byte, &body)
        }

        MqttPacket::PubAck {
            packet_id,
            reason_code,
            properties,
        } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 && (*reason_code != 0 || !properties.is_empty()) {
                body.push(*reason_code);
                if !properties.is_empty() {
                    body.extend_from_slice(&properties.encode());
                }
            }
            build_fixed_header(0x40, &body)
        }

        MqttPacket::PubRec {
            packet_id,
            reason_code,
        } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 && *reason_code != 0 {
                body.push(*reason_code);
            }
            build_fixed_header(0x50, &body)
        }

        MqttPacket::PubRel {
            packet_id,
            reason_code,
        } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 && *reason_code != 0 {
                body.push(*reason_code);
            }
            build_fixed_header(0x62, &body)
        }

        MqttPacket::PubComp {
            packet_id,
            reason_code,
        } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 && *reason_code != 0 {
                body.push(*reason_code);
            }
            build_fixed_header(0x70, &body)
        }

        MqttPacket::Subscribe {
            packet_id,
            topics,
            properties,
        } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 {
                body.extend_from_slice(&properties.encode());
            }
            for (topic, qos) in topics {
                write_mqtt_string(&mut body, topic);
                body.push(qos.as_u8());
            }
            build_fixed_header(0x82, &body)
        }

        MqttPacket::Unsubscribe { packet_id, topics } => {
            let mut body = Vec::new();
            body.extend_from_slice(&packet_id.to_be_bytes());
            if version == MqttVersion::V5 {
                body.push(0x00); // Empty properties
            }
            for topic in topics {
                write_mqtt_string(&mut body, topic);
            }
            build_fixed_header(0xA2, &body)
        }

        MqttPacket::PingReq => vec![0xC0, 0x00],
        MqttPacket::PingResp => vec![0xD0, 0x00],

        MqttPacket::Disconnect {
            reason_code,
            properties,
        } => {
            if version == MqttVersion::V5 && (*reason_code != 0 || !properties.is_empty()) {
                let mut body = vec![*reason_code];
                if !properties.is_empty() {
                    body.extend_from_slice(&properties.encode());
                }
                build_fixed_header(0xE0, &body)
            } else {
                vec![0xE0, 0x00]
            }
        }

        _ => Vec::new(),
    }
}

pub fn decode_packet(
    first_byte: u8,
    body: &[u8],
    version: MqttVersion,
) -> io::Result<MqttPacket> {
    let packet_type = first_byte >> 4;
    let flags = first_byte & 0x0F;
    let mut cur = Cursor::new(body);

    match packet_type {
        2 => {
            // CONNACK
            let ack_flags = read_u8(&mut cur)?;
            let session_present = (ack_flags & 0x01) != 0;
            let return_code = read_u8(&mut cur)?;
            let properties = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                Mqtt5Properties::decode(&mut cur)?
            } else {
                Mqtt5Properties::default()
            };
            Ok(MqttPacket::ConnAck {
                session_present,
                return_code,
                properties,
            })
        }

        3 => {
            // PUBLISH
            let dup = (flags & 0x08) != 0;
            let qos_val = (flags >> 1) & 0x03;
            let retain = (flags & 0x01) != 0;
            let qos = QoS::from_u8(qos_val)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            let topic = read_mqtt_string(&mut cur)?;
            let packet_id = if qos != QoS::AtMostOnce {
                Some(read_u16(&mut cur)?)
            } else {
                None
            };

            let properties = if version == MqttVersion::V5 {
                Mqtt5Properties::decode(&mut cur)?
            } else {
                Mqtt5Properties::default()
            };

            let pos = cur.position() as usize;
            let payload = body[pos..].to_vec();

            Ok(MqttPacket::Publish(PublishPacket {
                dup,
                qos,
                retain,
                topic,
                packet_id,
                payload,
                properties,
            }))
        }

        4 => {
            // PUBACK
            let packet_id = read_u16(&mut cur)?;
            let reason_code = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                read_u8(&mut cur)?
            } else {
                0
            };
            let properties = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                Mqtt5Properties::decode(&mut cur)?
            } else {
                Mqtt5Properties::default()
            };
            Ok(MqttPacket::PubAck {
                packet_id,
                reason_code,
                properties,
            })
        }

        5 => {
            // PUBREC
            let packet_id = read_u16(&mut cur)?;
            let reason_code = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                read_u8(&mut cur)?
            } else {
                0
            };
            Ok(MqttPacket::PubRec {
                packet_id,
                reason_code,
            })
        }

        6 => {
            // PUBREL
            let packet_id = read_u16(&mut cur)?;
            let reason_code = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                read_u8(&mut cur)?
            } else {
                0
            };
            Ok(MqttPacket::PubRel {
                packet_id,
                reason_code,
            })
        }

        7 => {
            // PUBCOMP
            let packet_id = read_u16(&mut cur)?;
            let reason_code = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                read_u8(&mut cur)?
            } else {
                0
            };
            Ok(MqttPacket::PubComp {
                packet_id,
                reason_code,
            })
        }

        9 => {
            // SUBACK
            let packet_id = read_u16(&mut cur)?;
            let properties = if version == MqttVersion::V5 {
                Mqtt5Properties::decode(&mut cur)?
            } else {
                Mqtt5Properties::default()
            };
            let pos = cur.position() as usize;
            let return_codes = body[pos..].to_vec();
            Ok(MqttPacket::SubAck {
                packet_id,
                return_codes,
                properties,
            })
        }

        11 => {
            // UNSUBACK
            let packet_id = read_u16(&mut cur)?;
            Ok(MqttPacket::UnsubAck { packet_id })
        }

        12 => Ok(MqttPacket::PingReq),
        13 => Ok(MqttPacket::PingResp),

        14 => {
            // DISCONNECT
            let reason_code = if version == MqttVersion::V5 && !body.is_empty() {
                read_u8(&mut cur)?
            } else {
                0
            };
            let properties = if version == MqttVersion::V5 && (cur.position() as usize) < body.len()
            {
                Mqtt5Properties::decode(&mut cur)?
            } else {
                Mqtt5Properties::default()
            };
            Ok(MqttPacket::Disconnect {
                reason_code,
                properties,
            })
        }

        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Unsupported MQTT packet type: {}", other),
        )),
    }
}

pub fn connack_error_description(code: u8, version: MqttVersion) -> String {
    if version == MqttVersion::V5 {
        match code {
            0x00 => "Success".to_string(),
            0x80 => "Unspecified error".to_string(),
            0x81 => "Malformed Packet".to_string(),
            0x82 => "Protocol Error".to_string(),
            0x83 => "Implementation specific error".to_string(),
            0x84 => "Unsupported Protocol Version".to_string(),
            0x85 => "Client Identifier not valid".to_string(),
            0x86 => "Bad User Name or Password".to_string(),
            0x87 => "Not authorized".to_string(),
            0x88 => "Server unavailable".to_string(),
            0x89 => "Server busy".to_string(),
            0x8A => "Banned".to_string(),
            0x8C => "Bad authentication method".to_string(),
            0x90 => "Topic Name invalid".to_string(),
            0x95 => "Packet too large".to_string(),
            0x97 => "Quota exceeded".to_string(),
            0x99 => "Payload format invalid".to_string(),
            0x9C => "Use another server".to_string(),
            0x9D => "Server moved".to_string(),
            0x9F => "Connection rate exceeded".to_string(),
            other => format!("MQTT v5 Reason Code 0x{:02X}", other),
        }
    } else {
        match code {
            0 => "Connection Accepted".to_string(),
            1 => "Connection Refused: Unacceptable protocol version".to_string(),
            2 => "Connection Refused: Identifier rejected".to_string(),
            3 => "Connection Refused: Server unavailable".to_string(),
            4 => "Connection Refused: Bad user name or password".to_string(),
            5 => "Connection Refused: Not authorized".to_string(),
            other => format!("Connection Refused: Code {}", other),
        }
    }
}

fn build_fixed_header(first_byte: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(first_byte);
    write_var_int(&mut out, body.len());
    out.extend_from_slice(body);
    out
}

pub fn write_var_int(buf: &mut Vec<u8>, mut len: usize) {
    loop {
        let mut encoded_byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            encoded_byte |= 128;
        }
        buf.push(encoded_byte);
        if len == 0 {
            break;
        }
    }
}

pub fn read_var_int(cur: &mut Cursor<&[u8]>) -> io::Result<usize> {
    let mut multiplier: usize = 1;
    let mut value: usize = 0;
    loop {
        let encoded_byte = read_u8(cur)?;
        value += ((encoded_byte & 127) as usize) * multiplier;
        if (encoded_byte & 128) == 0 {
            break;
        }
        multiplier *= 128;
        if multiplier > 128 * 128 * 128 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Malformed Remaining Length variable integer",
            ));
        }
    }
    Ok(value)
}

fn write_mqtt_string(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    let len = bytes.len().min(u16::MAX as usize) as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&bytes[..len as usize]);
}

fn write_mqtt_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
    let len = bytes.len().min(u16::MAX as usize) as u16;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&bytes[..len as usize]);
}

fn read_u8(cur: &mut Cursor<&[u8]>) -> io::Result<u8> {
    let mut b = [0u8; 1];
    cur.read_exact(&mut b)?;
    Ok(b[0])
}

fn read_u16(cur: &mut Cursor<&[u8]>) -> io::Result<u16> {
    let mut b = [0u8; 2];
    cur.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(cur: &mut Cursor<&[u8]>) -> io::Result<u32> {
    let mut b = [0u8; 4];
    cur.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_mqtt_string(cur: &mut Cursor<&[u8]>) -> io::Result<String> {
    let bytes = read_mqtt_bytes(cur)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn read_mqtt_bytes(cur: &mut Cursor<&[u8]>) -> io::Result<Vec<u8>> {
    let len = read_u16(cur)? as usize;
    let mut buf = vec![0u8; len];
    cur.read_exact(&mut buf)?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mqtt_v31_v311_v5_connect_encoding() {
        for version in [MqttVersion::V31, MqttVersion::V311, MqttVersion::V5] {
            let params = ConnectParams {
                version,
                client_id: "test-client-1".to_string(),
                clean_session: true,
                keep_alive_secs: 60,
                username: Some("alice".to_string()),
                password: Some("secret".to_string()),
                last_will: None,
                properties: Mqtt5Properties::default(),
            };
            let pkt = encode_packet(&MqttPacket::Connect(params), version);
            assert_eq!(pkt[0], 0x10);
            assert!(pkt.len() > 15);
        }
    }

    #[test]
    fn test_mqtt_v5_publish_roundtrip() {
        let mut props = Mqtt5Properties::default();
        props.content_type = Some("application/json".to_string());
        props.response_topic = Some("reply/topic".to_string());
        props.correlation_data = Some(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        props.message_expiry_interval = Some(3600);
        props
            .user_properties
            .push(("region".to_string(), "eu-central-1".to_string()));

        let publish = PublishPacket {
            dup: false,
            qos: QoS::AtLeastOnce,
            retain: true,
            topic: "sensors/factory/temp".to_string(),
            packet_id: Some(42),
            payload: br#"{"temp":22.5}"#.to_vec(),
            properties: props.clone(),
        };

        let raw = encode_packet(&MqttPacket::Publish(publish), MqttVersion::V5);
        let first_byte = raw[0];
        let mut cur = Cursor::new(&raw[1..]);
        let rem_len = read_var_int(&mut cur).unwrap();
        let body_start = 1 + cur.position() as usize;
        let decoded = decode_packet(
            first_byte,
            &raw[body_start..body_start + rem_len],
            MqttVersion::V5,
        )
        .unwrap();

        match decoded {
            MqttPacket::Publish(p) => {
                assert_eq!(p.topic, "sensors/factory/temp");
                assert_eq!(p.qos, QoS::AtLeastOnce);
                assert!(p.retain);
                assert_eq!(p.packet_id, Some(42));
                assert_eq!(p.payload, br#"{"temp":22.5}"#);
                assert_eq!(p.properties.content_type.as_deref(), Some("application/json"));
                assert_eq!(p.properties.response_topic.as_deref(), Some("reply/topic"));
                assert_eq!(
                    p.properties.correlation_data.as_deref(),
                    Some(&[0xDE, 0xAD, 0xBE, 0xEF][..])
                );
                assert_eq!(p.properties.message_expiry_interval, Some(3600));
                assert_eq!(
                    p.properties.user_properties,
                    vec![("region".to_string(), "eu-central-1".to_string())]
                );
            }
            _ => panic!("Expected Publish packet"),
        }
    }
}
