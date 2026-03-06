//! MOM SurrealDB Store - Multi-model persistence layer
//!
//! Leverages SurrealDB's document model, relationships, and queries
//! for efficient memory storage and hybrid retrieval.

use mom_core::{Content, MemoryId, MemoryItem, MemoryKind, Query, Scored};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use surrealdb::engine::local::Db;
use surrealdb::sql::Thing;
use surrealdb::Surreal;
use tracing::debug;

pub mod hybrid;

pub struct SurrealDBStore {
    db: Arc<Surreal<Db>>,
    namespace: String,
    database: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct StoredItem {
    id: String,
    tenant_id: String,
    workspace_id: Option<String>,
    project_id: Option<String>,
    agent_id: Option<String>,
    run_id: Option<String>,

    kind: String,
    created_at_ms: i64,

    content_text: Option<String>,
    content_json: Option<serde_json::Value>,

    importance: f32,
    confidence: f32,
    source: String,
    ttl_ms: Option<i64>,
    meta: serde_json::Value,

    tags: Vec<String>,

    // Phase 2: Vector embeddings
    #[serde(skip_serializing_if = "Option::is_none")]
    embedding: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    embedding_model: Option<String>,
}

impl SurrealDBStore {
    pub async fn new(_db_path: &str) -> anyhow::Result<Self> {
        // For in-memory backend, create new Surreal instance
        let db = Surreal::new::<Db>(()).await?;
        db.use_ns("mom").use_db("main").await?;

        Self::init_schema(&db).await?;

        Ok(Self {
            db: Arc::new(db),
            namespace: "mom".to_string(),
            database: "main".to_string(),
        })
    }

    async fn init_schema(db: &Surreal<Db>) -> anyhow::Result<()> {
        // Create table for memory items
        db.query(
            r#"
            DEFINE TABLE memory_items SCHEMAFULL PERMISSIONS
              FOR select WHERE tenant_id = $scope_tenant_id;
            DEFINE FIELD id ON TABLE memory_items TYPE string ASSERT string::len($value) > 0;
            DEFINE FIELD tenant_id ON TABLE memory_items TYPE string ASSERT string::len($value) > 0;
            DEFINE FIELD workspace_id ON TABLE memory_items TYPE option<string>;
            DEFINE FIELD project_id ON TABLE memory_items TYPE option<string>;
            DEFINE FIELD agent_id ON TABLE memory_items TYPE option<string>;
            DEFINE FIELD run_id ON TABLE memory_items TYPE option<string>;
            DEFINE FIELD kind ON TABLE memory_items TYPE string ASSERT $value IN ['Event', 'Summary', 'Fact', 'Preference'];
            DEFINE FIELD created_at_ms ON TABLE memory_items TYPE number;
            DEFINE FIELD content_text ON TABLE memory_items TYPE option<string>;
            DEFINE FIELD content_json ON TABLE memory_items TYPE option<object>;
            DEFINE FIELD importance ON TABLE memory_items TYPE number ASSERT $value >= 0 AND $value <= 1;
            DEFINE FIELD confidence ON TABLE memory_items TYPE number ASSERT $value >= 0 AND $value <= 1;
            DEFINE FIELD source ON TABLE memory_items TYPE string;
            DEFINE FIELD ttl_ms ON TABLE memory_items TYPE option<number>;
            DEFINE FIELD meta ON TABLE memory_items TYPE object;
            DEFINE FIELD tags ON TABLE memory_items TYPE array<string>;
            DEFINE FIELD embedding ON TABLE memory_items TYPE option<array<float>>;
            DEFINE FIELD embedding_model ON TABLE memory_items TYPE option<string>;

            DEFINE INDEX idx_tenant_time ON TABLE memory_items COLUMNS tenant_id, created_at_ms;
            DEFINE INDEX idx_scope ON TABLE memory_items COLUMNS tenant_id, workspace_id, project_id, agent_id, run_id;
            DEFINE INDEX idx_embedding ON TABLE memory_items COLUMNS embedding;
            "#
        )
        .await?;

        debug!("SurrealDB schema initialized");
        Ok(())
    }

    fn kind_to_str(k: MemoryKind) -> &'static str {
        match k {
            MemoryKind::Event => "Event",
            MemoryKind::Summary => "Summary",
            MemoryKind::Fact => "Fact",
            MemoryKind::Preference => "Preference",
        }
    }

    fn str_to_kind(s: &str) -> Option<MemoryKind> {
        match s {
            "Event" => Some(MemoryKind::Event),
            "Summary" => Some(MemoryKind::Summary),
            "Fact" => Some(MemoryKind::Fact),
            "Preference" => Some(MemoryKind::Preference),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl mom_core::MemoryStore for SurrealDBStore {
    async fn put(&self, item: MemoryItem) -> anyhow::Result<()> {
        let (content_text, content_json) = match &item.content {
            Content::Text(t) => (Some(t.clone()), None),
            Content::Json(v) => (None, Some(v.clone())),
            Content::TextJson { text, json } => (Some(text.clone()), Some(json.clone())),
        };

        let stored = StoredItem {
            id: item.id.0.clone(),
            tenant_id: item.scope.tenant_id.clone(),
            workspace_id: item.scope.workspace_id.clone(),
            project_id: item.scope.project_id.clone(),
            agent_id: item.scope.agent_id.clone(),
            run_id: item.scope.run_id.clone(),
            kind: Self::kind_to_str(item.kind).to_string(),
            created_at_ms: item.created_at_ms,
            content_text,
            content_json,
            importance: item.importance,
            confidence: item.confidence,
            source: item.source.clone(),
            ttl_ms: item.ttl_ms,
            meta: serde_json::to_value(&item.meta)?,
            tags: item.tags.clone(),
            embedding: item.embedding.clone(),
            embedding_model: item.embedding_model.clone(),
        };

        // Upsert using MERGE statement
        let query = format!(
            "UPSERT memory_items:{} MERGE {}",
            item.id.0,
            serde_json::to_string(&stored)?
        );

        let _: Vec<StoredItem> = self.db.query(&query).await?.take(0)?;

        debug!("Stored memory item: {}", item.id.0);
        Ok(())
    }

    async fn get(&self, id: &MemoryId) -> anyhow::Result<Option<MemoryItem>> {
        let query = format!("SELECT * FROM memory_items:{}", id.0);
        let results: Vec<StoredItem> = self.db.query(&query).await?.take(0)?;

        Ok(results.into_iter().next().map(|s| {
            let content = match (s.content_text, s.content_json) {
                (Some(text), None) => Content::Text(text),
                (None, Some(json)) => Content::Json(json),
                (Some(text), Some(json)) => Content::TextJson { text, json },
                _ => Content::Text(String::new()),
            };

            let kind = Self::str_to_kind(&s.kind).unwrap_or(MemoryKind::Event);

            MemoryItem {
                id: MemoryId(s.id),
                scope: mom_core::ScopeKey {
                    tenant_id: s.tenant_id,
                    workspace_id: s.workspace_id,
                    project_id: s.project_id,
                    agent_id: s.agent_id,
                    run_id: s.run_id,
                },
                kind,
                created_at_ms: s.created_at_ms,
                content,
                tags: s.tags,
                importance: s.importance,
                confidence: s.confidence,
                source: s.source,
                ttl_ms: s.ttl_ms,
                meta: serde_json::from_value(s.meta).unwrap_or_default(),
                embedding: s.embedding,
                embedding_model: s.embedding_model,
            }
        }))
    }

    async fn query(&self, q: Query) -> anyhow::Result<Vec<Scored<MemoryItem>>> {
        // Build SurrealQL query with tenant filter + optional refinements
        let mut query_str = format!(
            "SELECT * FROM memory_items WHERE tenant_id = '{}'",
            &q.scope.tenant_id
        );

        // Scope refinement
        if let Some(ref ws) = q.scope.workspace_id {
            query_str.push_str(&format!(" AND workspace_id = '{}'", ws));
        }
        if let Some(ref proj) = q.scope.project_id {
            query_str.push_str(&format!(" AND project_id = '{}'", proj));
        }
        if let Some(ref agent) = q.scope.agent_id {
            query_str.push_str(&format!(" AND agent_id = '{}'", agent));
        }

        // Kind filter
        if let Some(kinds) = &q.kinds {
            let kind_strs: Vec<_> = kinds.iter().map(|k| Self::kind_to_str(*k)).collect();
            let kinds_clause = kind_strs
                .iter()
                .map(|k| format!("'{}'", k))
                .collect::<Vec<_>>()
                .join(", ");
            query_str.push_str(&format!(" AND kind IN [{}]", kinds_clause));
        }

        // Time bounds
        if let Some(since) = q.since_ms {
            query_str.push_str(&format!(" AND created_at_ms >= {}", since));
        }
        if let Some(until) = q.until_ms {
            query_str.push_str(&format!(" AND created_at_ms <= {}", until));
        }

        // Text match (simple substring for MVP; enhance with FTS later)
        if !q.text.is_empty() {
            query_str.push_str(&format!(
                " AND (content_text CONTAINS '{}' OR tags CONTAINS ['{}'])",
                &q.text, &q.text
            ));
        }

        // Sort by importance + recency, limit
        query_str.push_str(&format!(
            " ORDER BY importance DESC, created_at_ms DESC LIMIT {}",
            q.limit
        ));

        let results: Vec<StoredItem> = self.db.query(&query_str).await?.take(0)?;

        let mut scored = Vec::with_capacity(results.len());
        for (idx, item) in results.into_iter().enumerate() {
            // Simple scoring: importance + recency bonus
            let recency_bonus = (1.0 - (idx as f32 / q.limit as f32).min(1.0)) * 0.2;
            let score = (item.importance + recency_bonus).min(1.0);

            let content = match (item.content_text, item.content_json) {
                (Some(text), None) => Content::Text(text),
                (None, Some(json)) => Content::Json(json),
                (Some(text), Some(json)) => Content::TextJson { text, json },
                _ => Content::Text(String::new()),
            };

            let kind = Self::str_to_kind(&item.kind).unwrap_or(MemoryKind::Event);

            scored.push(Scored {
                score,
                item: MemoryItem {
                    id: MemoryId(item.id),
                    scope: mom_core::ScopeKey {
                        tenant_id: item.tenant_id,
                        workspace_id: item.workspace_id,
                        project_id: item.project_id,
                        agent_id: item.agent_id,
                        run_id: item.run_id,
                    },
                    kind,
                    created_at_ms: item.created_at_ms,
                    content,
                    tags: item.tags,
                    importance: item.importance,
                    confidence: item.confidence,
                    source: item.source,
                    ttl_ms: item.ttl_ms,
                    meta: serde_json::from_value(item.meta).unwrap_or_default(),
                    embedding: item.embedding,
                    embedding_model: item.embedding_model,
                },
            });
        }

        debug!("Query found {} results", scored.len());
        Ok(scored)
    }

    async fn delete(&self, id: &MemoryId) -> anyhow::Result<()> {
        let query = format!("DELETE memory_items:{}", id.0);
        let _: Vec<StoredItem> = self.db.query(&query).await?.take(0)?;
        debug!("Deleted memory item: {}", id.0);
        Ok(())
    }
}
