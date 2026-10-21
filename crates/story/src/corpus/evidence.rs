//! 证据侧表。重复的状态和资源路径共享字符串，资源仅保留回查位置与核验摘要。
use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize)]
pub struct EvidenceCounts {
    pub placeholders: usize,
    pub unknown: usize,
    pub inferred: usize,
    pub missing: usize,
    pub unlinked: usize,
    pub no_voice: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceEvidence {
    #[serde(default)]
    pub assets: Vec<Arc<str>>,
    #[serde(default)]
    pub occurrences: Vec<ResourceLocation>,
    #[serde(default)]
    pub timelines: Vec<Arc<str>>,
    pub text_key: Option<Arc<str>>,
    pub avatar: Option<Arc<str>>,
    #[serde(default)]
    pub voice_keys: Vec<Arc<str>>,
    pub text_difference: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ResourceLocation {
    pub asset: Option<Arc<str>>,
    pub section: Option<u32>,
    pub node: Option<u32>,
    pub node_type: Option<Arc<str>>,
    pub text_field: Option<Arc<str>>,
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LineEvidence {
    pub content_status: Option<Arc<str>>,
    pub content_evidence: Option<Box<Value>>,
    pub source_attribution: Option<Box<Value>>,
    pub speaker_status: Option<Arc<str>>,
    pub speaker_gap_reason: Option<Arc<str>>,
    pub speaker_source: Option<Arc<str>>,
    pub speaker_slot: Option<Box<Value>>,
    pub has_voice: Option<bool>,
    pub line_order_source: Option<Arc<str>>,
    pub line_order_status: Option<Arc<str>>,
    pub perform: Option<Box<ResourceEvidence>>,
    pub timeline: Option<Box<ResourceEvidence>>,
}

impl LineEvidence {
    pub fn provided(&self) -> bool {
        self.content_status.is_some()
            || self.content_evidence.is_some()
            || self.source_attribution.is_some()
            || self.speaker_status.is_some()
            || self.speaker_source.is_some()
            || self.speaker_gap_reason.is_some()
            || self.speaker_slot.is_some()
            || self.has_voice.is_some()
            || self.line_order_source.is_some()
            || self.line_order_status.is_some()
            || self.perform.is_some()
            || self.timeline.is_some()
    }

    pub fn unlinked_candidate(&self) -> bool {
        self.speaker_status.as_deref() == Some("unresolved")
            && self.speaker_gap_reason.as_deref() == Some("no_structured_reference")
    }

    pub(crate) fn intern(&mut self, strings: &mut HashMap<Arc<str>, Arc<str>>) {
        fn intern(value: &mut Option<Arc<str>>, strings: &mut HashMap<Arc<str>, Arc<str>>) {
            if let Some(s) = value {
                *s = strings
                    .entry(s.clone())
                    .or_insert_with(|| s.clone())
                    .clone();
            }
        }
        for field in [
            &mut self.content_status,
            &mut self.speaker_status,
            &mut self.speaker_gap_reason,
            &mut self.speaker_source,
            &mut self.line_order_source,
            &mut self.line_order_status,
        ] {
            intern(field, strings);
        }
        for resource in [&mut self.perform, &mut self.timeline]
            .into_iter()
            .flatten()
        {
            intern(&mut resource.text_key, strings);
            intern(&mut resource.avatar, strings);
            for asset in resource
                .assets
                .iter_mut()
                .chain(&mut resource.timelines)
                .chain(&mut resource.voice_keys)
            {
                *asset = strings
                    .entry(asset.clone())
                    .or_insert_with(|| asset.clone())
                    .clone();
            }
            for location in &mut resource.occurrences {
                intern(&mut location.asset, strings);
                intern(&mut location.node_type, strings);
                intern(&mut location.text_field, strings);
            }
        }
    }
}
