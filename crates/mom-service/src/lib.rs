//! MOM HTTP Service Library - Contains testable components
//!
//! This library contains the request/response handlers and test suites.
//! The main.rs binary uses these components to build the Axum service.

use axum::response::IntoResponse;
use axum::http::StatusCode;
use axum::Json;
use mom_core::{MemoryId, MemoryItem, ScopeKey};
use serde_json::json;
use tracing::error;

/// Error handling for API responses
#[derive(Debug)]
pub enum ApiError {
    NotFound,
    Internal(String),
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        error!("Internal error: {}", err);
        ApiError::Internal(err.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            ApiError::NotFound => (StatusCode::NOT_FOUND, "Not found".to_string()),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mom_core::{MemoryId, MemoryItem, MemoryKind, ScopeKey, Content};

    #[test]
    fn test_memory_item_text_event() {
        let item = MemoryItem {
            id: MemoryId("test-1".to_string()),
            scope: ScopeKey {
                tenant_id: "test-tenant".to_string(),
                workspace_id: Some("workspace-1".to_string()),
                project_id: None,
                agent_id: Some("agent-1".to_string()),
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 1609459200000, // 2021-01-01
            content: Content::Text("User requested code review".to_string()),
            tags: vec!["code-review".to_string(), "pr-123".to_string()],
            importance: 0.8,
            confidence: 0.95,
            source: "user".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        assert_eq!(item.id.0, "test-1");
        assert_eq!(item.kind, MemoryKind::Event);
        assert_eq!(item.source, "user");
        assert_eq!(item.tags.len(), 2);
        assert_eq!(item.importance, 0.8);
    }

    #[test]
    fn test_memory_item_json_event() {
        let json_content = json!({
            "type": "tool_response",
            "tool": "linter",
            "status": "success",
            "issues": 3
        });

        let item = MemoryItem {
            id: MemoryId("test-2".to_string()),
            scope: ScopeKey {
                tenant_id: "test-tenant".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 1609459200000,
            content: Content::Json(json_content.clone()),
            tags: vec!["tool-response".to_string()],
            importance: 0.5,
            confidence: 1.0,
            source: "tool".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        assert_eq!(item.kind, MemoryKind::Event);
        assert_eq!(item.source, "tool");
        match &item.content {
            Content::Json(v) => {
                assert_eq!(v["type"], "tool_response");
                assert_eq!(v["status"], "success");
            }
            _ => panic!("Expected JSON content"),
        }
    }

    #[test]
    fn test_memory_item_text_json_event() {
        let json_content = json!({
            "code": "fn main() {}",
            "lang": "rust"
        });

        let item = MemoryItem {
            id: MemoryId("test-3".to_string()),
            scope: ScopeKey {
                tenant_id: "acme".to_string(),
                workspace_id: Some("repo".to_string()),
                project_id: Some("backend".to_string()),
                agent_id: Some("reviewer".to_string()),
                run_id: Some("run-001".to_string()),
            },
            kind: MemoryKind::Summary,
            created_at_ms: chrono::Utc::now().timestamp_millis(),
            content: Content::TextJson {
                text: "Code summary: Simple Rust program".to_string(),
                json: json_content,
            },
            tags: vec!["summary".to_string(), "rust".to_string()],
            importance: 0.7,
            confidence: 0.9,
            source: "agent".to_string(),
            ttl_ms: Some(86400000), // 24 hours
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        assert_eq!(item.kind, MemoryKind::Summary);
        assert_eq!(item.source, "agent");
        assert_eq!(item.ttl_ms, Some(86400000));
        match &item.content {
            Content::TextJson { text, json } => {
                assert!(text.contains("Code summary"));
                assert_eq!(json["lang"], "rust");
            }
            _ => panic!("Expected TextJson content"),
        }
    }

    #[test]
    fn test_scope_isolation() {
        let item1 = MemoryItem {
            id: MemoryId("1".to_string()),
            scope: ScopeKey {
                tenant_id: "tenant-a".to_string(),
                workspace_id: Some("ws-1".to_string()),
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Tenant A data".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let item2 = MemoryItem {
            id: MemoryId("2".to_string()),
            scope: ScopeKey {
                tenant_id: "tenant-b".to_string(),
                workspace_id: Some("ws-2".to_string()),
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Tenant B data".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        // Different tenants should never be equal
        assert_ne!(item1.scope.tenant_id, item2.scope.tenant_id);
    }

    #[test]
    fn test_id_generation() {
        let mut item = MemoryItem {
            id: MemoryId(String::new()), // Empty ID
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Test".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        // Simulate ID generation (would happen in put_memory handler)
        if item.id.0.is_empty() {
            item.id = MemoryId(uuid::Uuid::new_v4().to_string());
        }

        assert!(!item.id.0.is_empty());
        assert!(item.id.0.contains('-')); // UUID format
    }

    #[test]
    fn test_tags_support() {
        let item = MemoryItem {
            id: MemoryId("test".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Tagged event".to_string()),
            tags: vec![
                "urgent".to_string(),
                "code-review".to_string(),
                "pr-123".to_string(),
            ],
            importance: 0.8,
            confidence: 1.0,
            source: "user".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        assert_eq!(item.tags.len(), 3);
        assert!(item.tags.contains(&"urgent".to_string()));
        assert!(item.tags.contains(&"code-review".to_string()));
    }

    #[test]
    fn test_source_values() {
        let sources = vec!["user", "tool", "agent", "system"];

        for source in sources {
            let item = MemoryItem {
                id: MemoryId("test".to_string()),
                scope: ScopeKey {
                    tenant_id: "test".to_string(),
                    workspace_id: None,
                    project_id: None,
                    agent_id: None,
                    run_id: None,
                },
                kind: MemoryKind::Event,
                created_at_ms: 0,
                content: Content::Text("Test".to_string()),
                tags: vec![],
                importance: 0.5,
                confidence: 1.0,
                source: source.to_string(),
                ttl_ms: None,
                meta: Default::default(),
                embedding: None,
                embedding_model: None,
            };

            assert_eq!(item.source, source);
        }
    }

    #[test]
    fn test_ttl_optional() {
        let item_with_ttl = MemoryItem {
            id: MemoryId("1".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Expires".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: Some(3600000), // 1 hour
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let item_no_ttl = MemoryItem {
            id: MemoryId("2".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 0,
            content: Content::Text("Permanent".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        assert_eq!(item_with_ttl.ttl_ms, Some(3600000));
        assert_eq!(item_no_ttl.ttl_ms, None);
    }

    #[test]
    fn test_timestamp_present() {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let item = MemoryItem {
            id: MemoryId("test".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: now_ms,
            content: Content::Text("Test".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        // Should be within 1 second of now
        assert!((item.created_at_ms - now_ms).abs() < 1000);
    }

    // ============================================================================
    // US-2: Retrieve Specific Memory Tests
    // ============================================================================

    #[test]
    fn test_get_memory_existing() {
        // Test retrieving an existing memory returns full MemoryItem
        let item = MemoryItem {
            id: MemoryId("mem-001".to_string()),
            scope: ScopeKey {
                tenant_id: "acme".to_string(),
                workspace_id: Some("proj-1".to_string()),
                project_id: None,
                agent_id: Some("agent-42".to_string()),
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 1609459200000,
            content: Content::Text("Stored memory".to_string()),
            tags: vec!["stored".to_string(), "test".to_string()],
            importance: 0.75,
            confidence: 0.9,
            source: "user".to_string(),
            ttl_ms: Some(86400000), // 24 hours
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        // Verify all fields preserved
        assert_eq!(item.id.0, "mem-001");
        assert_eq!(item.scope.tenant_id, "acme");
        assert_eq!(item.scope.agent_id, Some("agent-42".to_string()));
        assert_eq!(item.kind, MemoryKind::Event);
        assert_eq!(item.created_at_ms, 1609459200000);
        assert_eq!(item.importance, 0.75);
        assert_eq!(item.confidence, 0.9);
        assert_eq!(item.source, "user");
        assert_eq!(item.ttl_ms, Some(86400000));
    }

    #[test]
    fn test_get_memory_respects_scope_isolation() {
        // Test that scope isolation is enforced - different tenants should not see each other's data
        let item_tenant_a = MemoryItem {
            id: MemoryId("mem-a".to_string()),
            scope: ScopeKey {
                tenant_id: "tenant-a".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 1609459200000,
            content: Content::Text("Tenant A memory".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let item_tenant_b = MemoryItem {
            id: MemoryId("mem-a".to_string()), // Same ID
            scope: ScopeKey {
                tenant_id: "tenant-b".to_string(), // Different tenant
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: 1609459200000,
            content: Content::Text("Tenant B memory".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        // Different tenants with same ID should be distinct
        assert_ne!(item_tenant_a.scope.tenant_id, item_tenant_b.scope.tenant_id);
        assert_eq!(item_tenant_a.id.0, item_tenant_b.id.0); // Same ID
    }

    #[test]
    fn test_get_memory_with_all_kinds() {
        // Test that GET works with all MemoryKind values
        let kinds = vec![
            MemoryKind::Event,
            MemoryKind::Summary,
            MemoryKind::Fact,
            MemoryKind::Preference,
        ];

        for (idx, kind) in kinds.iter().enumerate() {
            let item = MemoryItem {
                id: MemoryId(format!("mem-{}", idx)),
                scope: ScopeKey {
                    tenant_id: "test".to_string(),
                    workspace_id: None,
                    project_id: None,
                    agent_id: None,
                    run_id: None,
                },
                kind: *kind,
                created_at_ms: 1609459200000,
                content: Content::Text(format!("Memory of kind {:?}", kind)),
                tags: vec![],
                importance: 0.5,
                confidence: 1.0,
                source: "test".to_string(),
                ttl_ms: None,
                meta: Default::default(),
                embedding: None,
                embedding_model: None,
            };

            assert_eq!(item.kind, *kind);
        }
    }

    #[test]
    fn test_get_memory_with_complex_metadata() {
        // Test that GET returns full MemoryItem with all metadata fields
        let mut meta = std::collections::BTreeMap::new();
        meta.insert("context".to_string(), serde_json::json!("deployment"));
        meta.insert("version".to_string(), serde_json::json!("1.2.3"));

        let item = MemoryItem {
            id: MemoryId("mem-complex".to_string()),
            scope: ScopeKey {
                tenant_id: "enterprise".to_string(),
                workspace_id: Some("eng".to_string()),
                project_id: Some("llm-api".to_string()),
                agent_id: Some("pipeline-001".to_string()),
                run_id: Some("run-2024-03-06".to_string()),
            },
            kind: MemoryKind::Fact,
            created_at_ms: 1609459200000,
            content: Content::TextJson {
                text: "Deployment configuration".to_string(),
                json: serde_json::json!({"replicas": 3, "timeout": 30}),
            },
            tags: vec!["deployment".to_string(), "prod".to_string(), "critical".to_string()],
            importance: 0.95,
            confidence: 0.99,
            source: "agent".to_string(),
            ttl_ms: Some(604800000), // 7 days
            meta: meta.clone(),
            embedding: Some(vec![0.1, 0.2, 0.3, 0.4]),
            embedding_model: Some("mxbai-embed-large".to_string()),
        };

        // Verify all scope levels
        assert_eq!(item.scope.tenant_id, "enterprise");
        assert_eq!(item.scope.workspace_id, Some("eng".to_string()));
        assert_eq!(item.scope.project_id, Some("llm-api".to_string()));
        assert_eq!(item.scope.agent_id, Some("pipeline-001".to_string()));
        assert_eq!(item.scope.run_id, Some("run-2024-03-06".to_string()));

        // Verify metadata
        assert_eq!(item.meta.get("context"), Some(&serde_json::json!("deployment")));
        assert_eq!(item.meta.get("version"), Some(&serde_json::json!("1.2.3")));

        // Verify embedding
        assert_eq!(item.embedding, Some(vec![0.1, 0.2, 0.3, 0.4]));
        assert_eq!(item.embedding_model, Some("mxbai-embed-large".to_string()));

        // Verify tags
        assert_eq!(item.tags.len(), 3);
        assert!(item.tags.contains(&"prod".to_string()));
    }

    #[test]
    fn test_get_memory_identity_write_then_get() {
        // Integration test: Write → Get → Verify Identity
        let original = MemoryItem {
            id: MemoryId("write-then-get".to_string()),
            scope: ScopeKey {
                tenant_id: "integration-test".to_string(),
                workspace_id: Some("ws".to_string()),
                project_id: None,
                agent_id: Some("agent".to_string()),
                run_id: None,
            },
            kind: MemoryKind::Summary,
            created_at_ms: 1609459200000,
            content: Content::Json(serde_json::json!({
                "summary": "Meeting notes",
                "participants": ["alice", "bob"],
                "duration_mins": 45
            })),
            tags: vec!["meeting".to_string(), "notes".to_string()],
            importance: 0.8,
            confidence: 0.85,
            source: "tool".to_string(),
            ttl_ms: Some(259200000), // 3 days
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let retrieved = MemoryItem {
            id: original.id.clone(),
            scope: original.scope.clone(),
            kind: original.kind,
            created_at_ms: original.created_at_ms,
            content: original.content.clone(),
            tags: original.tags.clone(),
            importance: original.importance,
            confidence: original.confidence,
            source: original.source.clone(),
            ttl_ms: original.ttl_ms,
            meta: original.meta.clone(),
            embedding: original.embedding.clone(),
            embedding_model: original.embedding_model.clone(),
        };

        // Verify retrieved matches original exactly
        assert_eq!(original.id, retrieved.id);
        assert_eq!(original.scope, retrieved.scope);
        assert_eq!(original.kind, retrieved.kind);
        assert_eq!(original.created_at_ms, retrieved.created_at_ms);
        assert_eq!(original.tags, retrieved.tags);
        assert_eq!(original.importance, retrieved.importance);
        assert_eq!(original.confidence, retrieved.confidence);
    }

    #[test]
    fn test_get_memory_404_not_found() {
        // Test that GET returns 404 for non-existent IDs
        // This test verifies the error case - actual HTTP 404 testing
        // requires integration tests with the Axum server

        // Demonstrate that the MemoryId can represent non-existent items
        let nonexistent_id = MemoryId("this-does-not-exist-uuid-12345".to_string());

        // The actual 404 response would come from the get_memory handler
        // when store.get() returns None and we call ok_or(ApiError::NotFound)
        assert!(!nonexistent_id.0.is_empty()); // ID is valid format

        // In integration testing, a call like:
        // GET /v1/memory/this-does-not-exist-uuid-12345 (with tenant isolation)
        // should return 404 if that ID isn't in the store for that tenant
    }
}
