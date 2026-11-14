//! Hierarchical collapsible Topic Tree (MQTT Explorer style), message history,
//! rolling throughput metrics (msgs/min, msgs/sec, bytes/sec), root-topic share stats, and diff engine.

use crate::payload::{extract_numeric_value, make_compact_preview};
use crate::protocol::{Mqtt5Properties, QoS};
use chrono::{DateTime, Local};
use similar::{ChangeTag, TextDiff};
use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct TopicMessage {
    pub timestamp: DateTime<Local>,
    pub payload: Vec<u8>,
    pub qos: QoS,
    pub retain: bool,
    pub properties: Mqtt5Properties,
    pub compact_preview: String,
    pub numeric_value: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: DiffTag,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffTag {
    Equal,
    Delete,
    Insert,
}

#[derive(Debug, Clone)]
pub struct RootTopicStat {
    pub name: String,
    pub messages: u64,
    pub bytes: u64,
    pub fraction: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ThroughputBucket {
    pub sec_index: u64,
    pub messages: u64,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct TopicNode {
    pub segment: String,
    pub full_path: String,
    pub children: BTreeMap<String, TopicNode>,
    pub history: VecDeque<TopicMessage>,
    pub numeric_history: VecDeque<(f64, f64)>, // (relative_seconds, value)
    pub message_count: u64,
    pub total_subtree_messages: u64,
    pub total_subtree_bytes: u64,
    pub total_subtree_topics: usize,
    pub last_updated: Option<Instant>,
    pub expanded: bool,
}

impl TopicNode {
    pub fn new(segment: String, full_path: String, default_expanded: bool) -> Self {
        Self {
            segment,
            full_path,
            children: BTreeMap::new(),
            history: VecDeque::new(),
            numeric_history: VecDeque::new(),
            message_count: 0,
            total_subtree_messages: 0,
            total_subtree_bytes: 0,
            total_subtree_topics: 0,
            last_updated: None,
            expanded: default_expanded,
        }
    }

    pub fn latest_message(&self) -> Option<&TopicMessage> {
        self.history.front()
    }

    pub fn set_expanded_recursive(&mut self, expanded: bool) {
        if !self.full_path.is_empty() {
            self.expanded = expanded;
        }
        for child in self.children.values_mut() {
            child.set_expanded_recursive(expanded);
        }
    }

    pub fn matches_filter(&self, query_lower: &str) -> bool {
        if query_lower.is_empty() {
            return true;
        }
        if contains_ignore_case(&self.full_path, query_lower) {
            return true;
        }
        if let Some(msg) = self.latest_message() {
            if contains_ignore_case(&msg.compact_preview, query_lower) {
                return true;
            }
        }
        self.children
            .values()
            .any(|child| child.matches_filter(query_lower))
    }
}

/// Zero-allocation case-insensitive substring check for ASCII needles,
/// falling back to `to_lowercase()` only for non-ASCII queries.
fn contains_ignore_case(haystack: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle_lower.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle_lower.as_bytes();
        if n.len() > h.len() {
            return false;
        }
        return h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n));
    }
    haystack.to_lowercase().contains(needle_lower)
}

pub struct TopicTree {
    pub root: TopicNode,
    pub total_messages_received: u64,
    pub total_bytes_received: u64,
    pub total_unique_topics: usize,
    pub start_instant: Instant,
    pub buckets: VecDeque<ThroughputBucket>,
    pub peak_msgs_per_min: f64,
}

impl Default for TopicTree {
    fn default() -> Self {
        Self::new()
    }
}

impl TopicTree {
    pub fn new() -> Self {
        Self {
            root: TopicNode::new("root".to_string(), String::new(), true),
            total_messages_received: 0,
            total_bytes_received: 0,
            total_unique_topics: 0,
            start_instant: Instant::now(),
            buckets: VecDeque::new(),
            peak_msgs_per_min: 0.0,
        }
    }

    pub fn clear(&mut self) {
        self.root = TopicNode::new("root".to_string(), String::new(), true);
        self.total_messages_received = 0;
        self.total_bytes_received = 0;
        self.total_unique_topics = 0;
        self.start_instant = Instant::now();
        self.buckets.clear();
        self.peak_msgs_per_min = 0.0;
    }

    pub fn current_sec_index(&self) -> u64 {
        self.start_instant.elapsed().as_secs()
    }

    fn record_throughput(&mut self, bytes: u64) {
        let sec = self.current_sec_index();
        if let Some(back) = self.buckets.back_mut() {
            if back.sec_index == sec {
                back.messages += 1;
                back.bytes += bytes;
            } else {
                self.buckets.push_back(ThroughputBucket {
                    sec_index: sec,
                    messages: 1,
                    bytes,
                });
            }
        } else {
            self.buckets.push_back(ThroughputBucket {
                sec_index: sec,
                messages: 1,
                bytes,
            });
        }

        let cutoff = sec.saturating_sub(120);
        while let Some(front) = self.buckets.front() {
            if front.sec_index < cutoff {
                self.buckets.pop_front();
            } else {
                break;
            }
        }

        let mpm = self.messages_per_minute();
        if mpm > self.peak_msgs_per_min {
            self.peak_msgs_per_min = mpm;
        }
    }

    /// Total messages received in the last 60 seconds (messages / minute).
    pub fn messages_last_60s(&self) -> u64 {
        let now_sec = self.current_sec_index();
        let min_sec = now_sec.saturating_sub(59);
        self.buckets
            .iter()
            .filter(|b| b.sec_index >= min_sec && b.sec_index <= now_sec)
            .map(|b| b.messages)
            .sum()
    }

    /// Instantaneous / rolling Messages per Minute rate.
    pub fn messages_per_minute(&self) -> f64 {
        let last_60 = self.messages_last_60s() as f64;
        let elapsed = self.start_instant.elapsed().as_secs_f64().clamp(1.0, 60.0);
        if elapsed < 15.0 {
            // Blend actual 60s count with short-window rate so immediate bursts show clearly
            last_60.max((self.messages_last_10s() as f64) * 6.0)
        } else {
            last_60
        }
    }

    /// Messages received in the last 10 seconds.
    pub fn messages_last_10s(&self) -> u64 {
        let now_sec = self.current_sec_index();
        let min_sec = now_sec.saturating_sub(9);
        self.buckets
            .iter()
            .filter(|b| b.sec_index >= min_sec && b.sec_index <= now_sec)
            .map(|b| b.messages)
            .sum()
    }

    /// Smooth messages per second (averaged over the last 5 seconds).
    pub fn messages_per_second(&self) -> f64 {
        let now_sec = self.current_sec_index();
        let min_sec = now_sec.saturating_sub(4);
        let sum: u64 = self
            .buckets
            .iter()
            .filter(|b| b.sec_index >= min_sec && b.sec_index <= now_sec)
            .map(|b| b.messages)
            .sum();
        let window = (now_sec - min_sec + 1).clamp(1, 5) as f64;
        sum as f64 / window
    }

    /// Bytes received in the last 60 seconds.
    pub fn bytes_last_60s(&self) -> u64 {
        let now_sec = self.current_sec_index();
        let min_sec = now_sec.saturating_sub(59);
        self.buckets
            .iter()
            .filter(|b| b.sec_index >= min_sec && b.sec_index <= now_sec)
            .map(|b| b.bytes)
            .sum()
    }

    /// Returns 30 buckets (each covering 2 seconds of the last 60 seconds) of `(messages, bytes)`
    /// for rendering a clean 60-second throughput timeline.
    pub fn series_last_60s(&self, num_bars: usize) -> Vec<(u64, u64)> {
        let num_bars = num_bars.max(6);
        let now_sec = self.current_sec_index();
        let mut out = vec![(0u64, 0u64); num_bars];

        for b in &self.buckets {
            if b.sec_index <= now_sec && now_sec - b.sec_index < 60 {
                let age = (now_sec - b.sec_index) as usize; // 0..60
                let bar_from_right = (age * num_bars) / 60;
                let idx = num_bars.saturating_sub(1 + bar_from_right);
                out[idx].0 += b.messages;
                out[idx].1 += b.bytes;
            }
        }
        out
    }

    /// Breakdown of messages and bytes per root topic, sorted descending by message count.
    pub fn root_topic_stats(&self) -> Vec<RootTopicStat> {
        let total = self.total_messages_received.max(1) as f32;
        let mut stats: Vec<RootTopicStat> = self
            .root
            .children
            .values()
            .filter(|node| node.total_subtree_messages > 0)
            .map(|node| RootTopicStat {
                name: node.segment.clone(),
                messages: node.total_subtree_messages,
                bytes: node.total_subtree_bytes,
                fraction: (node.total_subtree_messages as f32) / total,
            })
            .collect();

        stats.sort_by(|a, b| b.messages.cmp(&a.messages));
        stats
    }

    pub fn insert_message(
        &mut self,
        topic: String,
        payload: Vec<u8>,
        qos: QoS,
        retain: bool,
        properties: Mqtt5Properties,
        max_history: usize,
    ) {
        let now_instant = Instant::now();
        let rel_secs = now_instant.duration_since(self.start_instant).as_secs_f64();
        let compact_preview = make_compact_preview(&payload);
        let numeric_value = extract_numeric_value(&payload);
        let payload_len = payload.len() as u64;

        self.total_messages_received += 1;
        self.total_bytes_received += payload_len;
        self.record_throughput(payload_len);

        let msg = TopicMessage {
            timestamp: Local::now(),
            payload,
            qos,
            retain,
            properties,
            compact_preview,
            numeric_value,
        };

        let segments: Vec<&str> = if topic.is_empty() {
            vec!["(empty)"]
        } else {
            topic.split('/').collect()
        };

        let is_new_topic = Self::insert_into_node(
            &mut self.root,
            &segments,
            0,
            String::new(),
            msg,
            payload_len,
            rel_secs,
            now_instant,
            max_history.max(2),
        );

        if is_new_topic {
            self.total_unique_topics += 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_into_node(
        node: &mut TopicNode,
        segments: &[&str],
        depth: usize,
        current_path: String,
        msg: TopicMessage,
        payload_len: u64,
        rel_secs: f64,
        now_instant: Instant,
        max_history: usize,
    ) -> bool {
        node.total_subtree_messages += 1;
        node.total_subtree_bytes += payload_len;
        node.last_updated = Some(now_instant);

        if depth == segments.len() {
            let was_new = node.message_count == 0;
            node.message_count += 1;
            if let Some(val) = msg.numeric_value {
                node.numeric_history.push_back((rel_secs, val));
                while node.numeric_history.len() > 120 {
                    node.numeric_history.pop_front();
                }
            }
            node.history.push_front(msg);
            while node.history.len() > max_history {
                node.history.pop_back();
            }
            return was_new;
        }

        let seg = segments[depth];
        let next_path = if current_path.is_empty() && depth == 0 {
            seg.to_string()
        } else {
            format!("{}/{}", current_path, seg)
        };

        let default_expanded = depth < 2;
        let child = node
            .children
            .entry(seg.to_string())
            .or_insert_with(|| TopicNode::new(seg.to_string(), next_path.clone(), default_expanded));

        let is_new = Self::insert_into_node(
            child,
            segments,
            depth + 1,
            next_path,
            msg,
            payload_len,
            rel_secs,
            now_instant,
            max_history,
        );
        if is_new {
            node.total_subtree_topics += 1;
        }
        is_new
    }

    pub fn find_node(&self, full_path: &str) -> Option<&TopicNode> {
        if full_path.is_empty() {
            return None;
        }
        let segments: Vec<&str> = full_path.split('/').collect();
        let mut cur = &self.root;
        for seg in segments {
            cur = cur.children.get(seg)?;
        }
        Some(cur)
    }
}

/// Compute a line-by-line diff between two formatted payload strings.
pub fn compute_payload_diff(old_text: &str, new_text: &str) -> Vec<DiffLine> {
    let diff = TextDiff::from_lines(old_text, new_text);
    let mut lines = Vec::new();
    for change in diff.iter_all_changes() {
        let tag = match change.tag() {
            ChangeTag::Equal => DiffTag::Equal,
            ChangeTag::Delete => DiffTag::Delete,
            ChangeTag::Insert => DiffTag::Insert,
        };
        lines.push(DiffLine {
            tag,
            text: change.value().trim_end_matches(&['\r', '\n'][..]).to_string(),
        });
    }
    lines
}
