//! MOM Core - Stable kernel API for event-sourced memory
//!
//! This is the minimal "MOM contract" - everything depends on it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct MemoryId(pub String);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    Event,
    Summary,
    Fact,
    Preference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScopeKey {
    pub tenant_id: String,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: MemoryId,
    pub scope: ScopeKey,
    pub kind: MemoryKind,
    pub created_at_ms: i64,
    pub content: Content,
    pub tags: Vec<String>,

    // ranking knobs
    pub importance: f32,   // 0..1
    pub confidence: f32,   // 0..1

    // provenance / safety
    pub source: String,    // "user" | "tool" | "agent" | "system"
    pub ttl_ms: Option<i64>,
    pub meta: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub scope: ScopeKey,
    pub text: String,
    pub kinds: Option<Vec<MemoryKind>>,
    pub tags_any: Option<Vec<String>>,
    pub limit: usize,

    // optional: time bounds (ms since epoch)
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scored<T> {
    pub score: f32,
    pub item: T,
}

/// Core storage trait - implement this for new backends
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    async fn put(&self, item: MemoryItem) -> anyhow::Result<()>;
    async fn get(&self, id: &MemoryId) -> anyhow::Result<Option<MemoryItem>>;
    async fn query(&self, q: Query) -> anyhow::Result<Vec<Scored<MemoryItem>>>;
    async fn delete(&self, id: &MemoryId) -> anyhow::Result<()>;
}

/// Optional: embedder for semantic search (plug in later)
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>>;
    fn dims(&self) -> usize;
    fn model_id(&self) -> &str;
}

impl MemoryItem {
    pub fn new(
        id: MemoryId,
        scope: ScopeKey,
        kind: MemoryKind,
        content: Content,
        source: String,
    ) -> Self {
        Self {
            id,
            scope,
            kind,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            content,
            tags: Vec::new(),
            importance: 0.5,
            confidence: 1.0,
            source,
            ttl_ms: None,
            meta: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_item_new() {
        let item = MemoryItem::new(
            MemoryId("test-1".to_string()),
            ScopeKey {
                tenant_id: "acme".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            MemoryKind::Event,
            Content::Text("Hello world".to_string()),
            "user".to_string(),
        );

        assert_eq!(item.id.0, "test-1");
        assert_eq!(item.kind, MemoryKind::Event);
        assert_eq!(item.importance, 0.5);
    }
}
