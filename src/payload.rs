//! Payload inspection, formatting, encoding/decoding, and syntax highlighting.
//! Supports: Auto-detect, JSON, UTF-8 Text, Hex Dump, Base64, MessagePack, and Protobuf wire format.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadFormat {
    Auto,
    Json,
    Text,
    Hex,
    Base64,
    MessagePack,
    Protobuf,
}

impl PayloadFormat {
    pub fn label(&self) -> &'static str {
        match self {
            PayloadFormat::Auto => "Auto",
            PayloadFormat::Json => "JSON",
            PayloadFormat::Text => "UTF-8 Text",
            PayloadFormat::Hex => "Hex",
            PayloadFormat::Base64 => "Base64",
            PayloadFormat::MessagePack => "MessagePack",
            PayloadFormat::Protobuf => "Protobuf Wire",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishEncoding {
    Text,
    Json,
    Hex,
    Base64,
    MessagePackFromJson,
    ProtobufFromJson,
}

impl PublishEncoding {
    pub fn label(&self) -> &'static str {
        match self {
            PublishEncoding::Text => "Raw / UTF-8",
            PublishEncoding::Json => "JSON (Validated)",
            PublishEncoding::Hex => "Hex String -> Bytes",
            PublishEncoding::Base64 => "Base64 -> Bytes",
            PublishEncoding::MessagePackFromJson => "JSON -> MessagePack (Binary)",
            PublishEncoding::ProtobufFromJson => "JSON -> Protobuf Wire (Binary)",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FormattedPayload {
    pub detected_format: PayloadFormat,
    pub display_text: String,
    pub is_json_highlightable: bool,
}

/// Format raw bytes according to the requested `PayloadFormat` (or auto-detect).
pub fn format_payload(bytes: &[u8], requested: PayloadFormat) -> FormattedPayload {
    let effective = if requested == PayloadFormat::Auto {
        detect_best_format(bytes)
    } else {
        requested
    };

    match effective {
        PayloadFormat::Json => {
            if let Ok(val) = serde_json::from_slice::<JsonValue>(bytes) {
                let pretty =
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| String::new());
                return FormattedPayload {
                    detected_format: PayloadFormat::Json,
                    display_text: pretty,
                    is_json_highlightable: true,
                };
            }
            // Fallback to UTF-8 text if invalid JSON
            let text = String::from_utf8_lossy(bytes).into_owned();
            FormattedPayload {
                detected_format: PayloadFormat::Json,
                display_text: format!("// (Invalid JSON, showing raw UTF-8)\n{}", text),
                is_json_highlightable: false,
            }
        }

        PayloadFormat::Text | PayloadFormat::Auto => {
            let text = String::from_utf8_lossy(bytes).into_owned();
            FormattedPayload {
                detected_format: PayloadFormat::Text,
                display_text: text,
                is_json_highlightable: false,
            }
        }

        PayloadFormat::Hex => FormattedPayload {
            detected_format: PayloadFormat::Hex,
            display_text: format_hex_dump(bytes),
            is_json_highlightable: false,
        },

        PayloadFormat::Base64 => {
            let encoded = BASE64.encode(bytes);
            let mut out = format!("// Base64 of raw payload ({} bytes):\n{}\n", bytes.len(), encoded);
            if let Ok(as_str) = std::str::from_utf8(bytes) {
                let trimmed = as_str.trim();
                if trimmed.len() >= 4 {
                    if let Ok(decoded) = BASE64.decode(trimmed) {
                        out.push_str("\n// Payload itself decoded from Base64:\n");
                        if let Ok(utf8) = String::from_utf8(decoded.clone()) {
                            out.push_str(&utf8);
                        } else {
                            out.push_str(&format_hex_dump(&decoded));
                        }
                    }
                }
            }
            FormattedPayload {
                detected_format: PayloadFormat::Base64,
                display_text: out,
                is_json_highlightable: false,
            }
        }

        PayloadFormat::MessagePack => match rmp_serde::from_slice::<JsonValue>(bytes) {
            Ok(val) => {
                let pretty =
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| format!("{:?}", val));
                FormattedPayload {
                    detected_format: PayloadFormat::MessagePack,
                    display_text: pretty,
                    is_json_highlightable: true,
                }
            }
            Err(e) => FormattedPayload {
                detected_format: PayloadFormat::MessagePack,
                display_text: format!(
                    "// Failed to decode as MessagePack: {}\n\n{}",
                    e,
                    format_hex_dump(bytes)
                ),
                is_json_highlightable: false,
            },
        },

        PayloadFormat::Protobuf => match decode_protobuf_wire(bytes, 0) {
            Ok(val) => {
                let pretty =
                    serde_json::to_string_pretty(&val).unwrap_or_else(|_| format!("{:?}", val));
                FormattedPayload {
                    detected_format: PayloadFormat::Protobuf,
                    display_text: pretty,
                    is_json_highlightable: true,
                }
            }
            Err(e) => FormattedPayload {
                detected_format: PayloadFormat::Protobuf,
                display_text: format!(
                    "// Not valid Protobuf wire format ({})\n\n{}",
                    e,
                    format_hex_dump(bytes)
                ),
                is_json_highlightable: false,
            },
        },
    }
}

pub fn detect_best_format(bytes: &[u8]) -> PayloadFormat {
    if bytes.is_empty() {
        return PayloadFormat::Text;
    }

    if let Ok(s) = std::str::from_utf8(bytes) {
        let trimmed = s.trim();
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            if serde_json::from_str::<JsonValue>(trimmed).is_ok() {
                return PayloadFormat::Json;
            }
        }
        // Check if printable UTF-8
        if s.chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
        {
            return PayloadFormat::Text;
        }
    }

    // Binary payload: check if valid MessagePack map/array
    let first = bytes[0];
    if (0x81..=0x8f).contains(&first)
        || (0x91..=0x9f).contains(&first)
        || first == 0xdc
        || first == 0xdd
        || first == 0xde
        || first == 0xdf
    {
        if rmp_serde::from_slice::<JsonValue>(bytes).is_ok() {
            return PayloadFormat::MessagePack;
        }
    }

    // Check if valid Protobuf wire format
    if decode_protobuf_wire(bytes, 0).is_ok() {
        return PayloadFormat::Protobuf;
    }

    PayloadFormat::Hex
}

pub fn make_compact_preview(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(empty)".to_string();
    }
    if let Ok(s) = std::str::from_utf8(bytes) {
        let single_line: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .take(80)
            .collect();
        let trimmed = single_line.trim();
        if !trimmed.is_empty() && trimmed.chars().all(|c| !c.is_control()) {
            return if s.len() > 80 {
                format!("{}...", trimmed)
            } else {
                trimmed.to_string()
            };
        }
    }
    if let Ok(val) = rmp_serde::from_slice::<JsonValue>(bytes) {
        if let Ok(s) = serde_json::to_string(&val) {
            let preview: String = s.chars().take(72).collect();
            return format!("[MsgPack] {}", preview);
        }
    }
    let hex_preview: String = bytes
        .iter()
        .take(16)
        .map(|b| format!("{:02x}", b))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > 16 {
        format!("[Hex] {} ...", hex_preview)
    } else {
        format!("[Hex] {}", hex_preview)
    }
}

pub fn extract_numeric_value(bytes: &[u8]) -> Option<f64> {
    if let Ok(s) = std::str::from_utf8(bytes) {
        let trimmed = s.trim();
        if let Ok(v) = trimmed.parse::<f64>() {
            if v.is_finite() {
                return Some(v);
            }
        }
        if let Ok(JsonValue::Object(map)) = serde_json::from_str::<JsonValue>(trimmed) {
            for (_, v) in map {
                if let Some(num) = v.as_f64() {
                    if num.is_finite() {
                        return Some(num);
                    }
                }
            }
        }
    }
    None
}

/// Canonical hex + ASCII dump (16 bytes per row)
pub fn format_hex_dump(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return "(0 bytes)".to_string();
    }
    let mut out = String::new();
    for (chunk_idx, chunk) in bytes.chunks(16).enumerate() {
        let offset = chunk_idx * 16;
        out.push_str(&format!("{:08x}  ", offset));

        for i in 0..16 {
            if i < chunk.len() {
                out.push_str(&format!("{:02x} ", chunk[i]));
            } else {
                out.push_str("   ");
            }
            if i == 7 {
                out.push(' ');
            }
        }

        out.push_str(" |");
        for &b in chunk {
            if (0x20..=0x7e).contains(&b) {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push_str("|\n");
    }
    out
}

/// Schema-less Protobuf Wire Format Decoder (recursive)
/// Unpacks field tags and wire types:
/// - 0: Varint (uint64 / int64 / bool)
/// - 1: 64-bit (fixed64 / double)
/// - 2: Length-delimited (UTF-8 string, nested message, or raw bytes)
/// - 5: 32-bit (fixed32 / float)
pub fn decode_protobuf_wire(bytes: &[u8], depth: usize) -> Result<JsonValue, String> {
    if bytes.is_empty() || depth > 6 {
        return Err("Empty or max depth exceeded".to_string());
    }

    let mut pos = 0;
    let mut map = serde_json::Map::new();
    let mut fields_decoded = 0;

    while pos < bytes.len() {
        let (tag, next_pos) = read_pb_varint(bytes, pos)?;
        pos = next_pos;
        let field_num = tag >> 3;
        let wire_type = tag & 0x07;

        if field_num == 0 || field_num > 536_870_911 {
            return Err(format!("Invalid protobuf field number: {}", field_num));
        }

        let key = format!("field_{}", field_num);
        let value = match wire_type {
            0 => {
                let (val, npos) = read_pb_varint(bytes, pos)?;
                pos = npos;
                JsonValue::Number(val.into())
            }
            1 => {
                if pos + 8 > bytes.len() {
                    return Err("Unexpected EOF in 64-bit field".to_string());
                }
                let raw: [u8; 8] = bytes[pos..pos + 8].try_into().unwrap();
                pos += 8;
                let uval = u64::from_le_bytes(raw);
                let fval = f64::from_le_bytes(raw);
                let mut obj = serde_json::Map::new();
                obj.insert("u64".to_string(), JsonValue::Number(uval.into()));
                if let Some(n) = serde_json::Number::from_f64(fval) {
                    obj.insert("f64".to_string(), JsonValue::Number(n));
                }
                JsonValue::Object(obj)
            }
            2 => {
                let (len, npos) = read_pb_varint(bytes, pos)?;
                let len = len as usize;
                pos = npos;
                if pos + len > bytes.len() {
                    return Err("Length-delimited field exceeds buffer".to_string());
                }
                let slice = &bytes[pos..pos + len];
                pos += len;

                if let Ok(s) = std::str::from_utf8(slice) {
                    if s.chars()
                        .all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
                    {
                        JsonValue::String(s.to_string())
                    } else if let Ok(nested) = decode_protobuf_wire(slice, depth + 1) {
                        nested
                    } else {
                        JsonValue::String(format!("0x{}", hex::encode(slice)))
                    }
                } else if let Ok(nested) = decode_protobuf_wire(slice, depth + 1) {
                    nested
                } else {
                    JsonValue::String(format!("0x{}", hex::encode(slice)))
                }
            }
            5 => {
                if pos + 4 > bytes.len() {
                    return Err("Unexpected EOF in 32-bit field".to_string());
                }
                let raw: [u8; 4] = bytes[pos..pos + 4].try_into().unwrap();
                pos += 4;
                let uval = u32::from_le_bytes(raw);
                let fval = f32::from_le_bytes(raw);
                let mut obj = serde_json::Map::new();
                obj.insert("u32".to_string(), JsonValue::Number(uval.into()));
                if let Some(n) = serde_json::Number::from_f64(fval as f64) {
                    obj.insert("f32".to_string(), JsonValue::Number(n));
                }
                JsonValue::Object(obj)
            }
            other => {
                return Err(format!("Unsupported protobuf wire type: {}", other));
            }
        };

        if let Some(existing) = map.get_mut(&key) {
            if let JsonValue::Array(arr) = existing {
                arr.push(value);
            } else {
                let prev = existing.clone();
                *existing = JsonValue::Array(vec![prev, value]);
            }
        } else {
            map.insert(key, value);
        }
        fields_decoded += 1;
    }

    if fields_decoded == 0 {
        return Err("No protobuf fields decoded".to_string());
    }

    Ok(JsonValue::Object(map))
}

fn read_pb_varint(bytes: &[u8], mut pos: usize) -> Result<(u64, usize), String> {
    let mut result: u64 = 0;
    let mut shift = 0;
    loop {
        if pos >= bytes.len() {
            return Err("Unexpected EOF reading varint".to_string());
        }
        let b = bytes[pos];
        pos += 1;
        result |= ((b & 0x7F) as u64) << shift;
        if (b & 0x80) == 0 {
            return Ok((result, pos));
        }
        shift += 7;
        if shift >= 64 {
            return Err("Protobuf varint overflow".to_string());
        }
    }
}

fn write_pb_varint(buf: &mut Vec<u8>, mut val: u64) {
    loop {
        if (val & !0x7F) == 0 {
            buf.push(val as u8);
            break;
        } else {
            buf.push(((val & 0x7F) as u8) | 0x80);
            val >>= 7;
        }
    }
}

/// Encode user input string into binary payload according to selected `PublishEncoding`.
pub fn encode_publish_payload(input: &str, mode: PublishEncoding) -> Result<Vec<u8>, String> {
    match mode {
        PublishEncoding::Text => Ok(input.as_bytes().to_vec()),
        PublishEncoding::Json => {
            let val: JsonValue = serde_json::from_str(input)
                .map_err(|e| format!("Invalid JSON payload: {}", e))?;
            serde_json::to_vec(&val).map_err(|e| format!("JSON serialization error: {}", e))
        }
        PublishEncoding::Hex => {
            let clean: String = input
                .chars()
                .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-')
                .collect();
            let clean = clean.strip_prefix("0x").unwrap_or(&clean);
            hex::decode(clean).map_err(|e| format!("Invalid Hex string: {}", e))
        }
        PublishEncoding::Base64 => BASE64
            .decode(input.trim())
            .map_err(|e| format!("Invalid Base64 string: {}", e)),
        PublishEncoding::MessagePackFromJson => {
            let val: JsonValue = serde_json::from_str(input)
                .map_err(|e| format!("Enter valid JSON to encode as MessagePack: {}", e))?;
            rmp_serde::to_vec_named(&val)
                .map_err(|e| format!("MessagePack encoding error: {}", e))
        }
        PublishEncoding::ProtobufFromJson => {
            let val: JsonValue = serde_json::from_str(input).map_err(|e| {
                format!(
                    "Enter JSON object (e.g. {{\"1\": 42, \"2\": \"hello\"}}) for Protobuf wire encoding: {}",
                    e
                )
            })?;
            encode_json_to_protobuf_wire(&val)
        }
    }
}

fn encode_json_to_protobuf_wire(val: &JsonValue) -> Result<Vec<u8>, String> {
    let obj = val
        .as_object()
        .ok_or_else(|| "Protobuf wire encoder expects a JSON object mapping field numbers (e.g. \"1\" or \"field_1\") to values".to_string())?;
    let mut out = Vec::new();
    for (idx, (k, v)) in obj.iter().enumerate() {
        let num_str = k.strip_prefix("field_").unwrap_or(k);
        let field_num: u64 = num_str.parse().unwrap_or((idx + 1) as u64);
        match v {
            JsonValue::Bool(b) => {
                write_pb_varint(&mut out, (field_num << 3) | 0);
                write_pb_varint(&mut out, if *b { 1 } else { 0 });
            }
            JsonValue::Number(n) => {
                if let Some(u) = n.as_u64() {
                    write_pb_varint(&mut out, (field_num << 3) | 0);
                    write_pb_varint(&mut out, u);
                } else if let Some(i) = n.as_i64() {
                    write_pb_varint(&mut out, (field_num << 3) | 0);
                    write_pb_varint(&mut out, i as u64);
                } else if let Some(f) = n.as_f64() {
                    write_pb_varint(&mut out, (field_num << 3) | 1);
                    out.extend_from_slice(&f.to_le_bytes());
                }
            }
            JsonValue::String(s) => {
                write_pb_varint(&mut out, (field_num << 3) | 2);
                write_pb_varint(&mut out, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            JsonValue::Object(_) => {
                let nested = encode_json_to_protobuf_wire(v)?;
                write_pb_varint(&mut out, (field_num << 3) | 2);
                write_pb_varint(&mut out, nested.len() as u64);
                out.extend_from_slice(&nested);
            }
            _ => {}
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_msgpack_roundtrip() {
        let json = r#"{"sensor":"living_room","temp":23.5,"active":true}"#;
        let encoded = encode_publish_payload(json, PublishEncoding::MessagePackFromJson).unwrap();
        let formatted = format_payload(&encoded, PayloadFormat::Auto);
        assert_eq!(formatted.detected_format, PayloadFormat::MessagePack);
        assert!(formatted.display_text.contains("living_room"));
    }

    #[test]
    fn test_protobuf_roundtrip() {
        let json = r#"{"field_1": 150, "field_2": "testing"}"#;
        let encoded = encode_publish_payload(json, PublishEncoding::ProtobufFromJson).unwrap();
        let formatted = format_payload(&encoded, PayloadFormat::Protobuf);
        assert!(formatted.display_text.contains("150"));
        assert!(formatted.display_text.contains("testing"));
    }
}
