//! Memory item types and structures

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub scope: ScopeKey,
    pub kind: MemoryKind,
    pub time: chrono::DateTime<chrono::Utc>,
    pub content: Content,
    pub tags: Vec<String>,
    pub links: Vec<MemoryLink>,
    pub importance: f32,
    pub confidence: f32,
    pub ttl: Option<chrono::Duration>,
    pub source: Source,
    pub integrity_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeKey {
    pub tenant_id: String,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Event,
    Episode,
    Summary,
    Fact,
    Preference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Json(serde_json::Value),
    TextJson {
        text: String,
        json: serde_json::Value,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    User,
    Tool,
    Model,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    pub src_id: String,
    pub dst_id: String,
    pub relation: String,
    pub weight: Option<f32>,
}

impl MemoryItem {
    pub fn new(
        scope: ScopeKey,
        kind: MemoryKind,
        content: Content,
        source: Source,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        Self {
            id,
            scope,
            kind,
            time: chrono::Utc::now(),
            content,
            tags: Vec::new(),
            links: Vec::new(),
            importance: 0.5,
            confidence: 1.0,
            ttl: None,
            source,
            integrity_hash: String::new(),
        }
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn compute_integrity_hash(&mut self) {
        // Placeholder: implement SHA256 hash in Phase 2
        self.integrity_hash = format!("{:x}", uuid::Uuid::new_v4());
    }
}
