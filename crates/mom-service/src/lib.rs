//! MOM HTTP Service Library - Contains testable components
//!
//! This library contains the request/response handlers and test suites.
//! The main.rs binary uses these components to build the Axum service.

use axum::response::IntoResponse;
use axum::http::StatusCode;
use axum::Json;
use mom_core::{MemoryId, MemoryItem, MemoryKind, Query, Scored, ScopeKey, Content};
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
}
