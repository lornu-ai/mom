use axum::{
    extract::{Json, Path},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use mom::{
    memory::{Content, MemoryItem, MemoryKind, ScopeKey, Source},
    store::{InMemoryStore, MemoryStore, Query},
    ContextPack, MomError, Result,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct AppState {
    store: Arc<InMemoryStore>,
}

#[derive(Debug, Serialize, Deserialize)]
struct WriteRequest {
    kind: String,
    content: serde_json::Value,
    tags: Option<Vec<String>>,
    scope: Option<ScopeKeyRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ScopeKeyRequest {
    tenant_id: Option<String>,
    workspace_id: Option<String>,
    project_id: Option<String>,
    agent_id: Option<String>,
    run_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RecallRequest {
    query: String,
    scope: Option<ScopeKeyRequest>,
    budget_tokens: Option<usize>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let store = Arc::new(InMemoryStore::new());
    let state = AppState { store };

    let app = Router::new()
        .route("/v1/memory", post(write_memory))
        .route("/v1/memory/:id", get(get_memory))
        .route("/v1/recall", post(recall_memory))
        .route("/health", get(health))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("Failed to bind address");

    tracing::info!("🧠 MOM server starting on http://127.0.0.1:8080");
    tracing::info!("📚 Endpoints:");
    tracing::info!("  POST   /v1/memory    - Write memory");
    tracing::info!("  GET    /v1/memory/:id - Get memory");
    tracing::info!("  POST   /v1/recall    - Recall context");

    axum::serve(app, listener)
        .await
        .expect("Server error");
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn write_memory(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<WriteRequest>,
) -> Result<(StatusCode, Json<MemoryItem>)> {
    let kind = match req.kind.as_str() {
        "event" => MemoryKind::Event,
        "episode" => MemoryKind::Episode,
        "summary" => MemoryKind::Summary,
        "fact" => MemoryKind::Fact,
        "preference" => MemoryKind::Preference,
        _ => return Err(MomError::InvalidInput("Invalid memory kind".to_string())),
    };

    let scope = req.scope.unwrap_or(ScopeKeyRequest {
        tenant_id: None,
        workspace_id: None,
        project_id: None,
        agent_id: None,
        run_id: None,
    });

    let scope_key = ScopeKey {
        tenant_id: scope.tenant_id.unwrap_or_else(|| "default".to_string()),
        workspace_id: scope.workspace_id,
        project_id: scope.project_id,
        agent_id: scope.agent_id,
        run_id: scope.run_id,
    };

    let content = match req.content {
        serde_json::Value::String(s) => Content::Text(s),
        serde_json::Value::Object(_) => Content::Json(req.content),
        _ => return Err(MomError::InvalidInput("Invalid content".to_string())),
    };

    let mut item = MemoryItem::new(scope_key, kind, content, Source::User);
    if let Some(tags) = req.tags {
        item = item.with_tags(tags);
    }
    item.compute_integrity_hash();

    state.store.put(&item).await?;

    tracing::debug!("Wrote memory: {}", item.id);

    Ok((StatusCode::CREATED, Json(item)))
}

async fn get_memory(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryItem>> {
    let item = state
        .store
        .get(&id)
        .await?
        .ok_or_else(|| MomError::NotFound(format!("Memory {} not found", id)))?;

    Ok(Json(item))
}

async fn recall_memory(
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(req): Json<RecallRequest>,
) -> Result<Json<ContextPack>> {
    let query = Query {
        text: Some(req.query.clone()),
        tags: None,
        kind: None,
        limit: Some(10),
        offset: Some(0),
    };

    let results = state.store.query(query).await?;

    let items: Vec<_> = results.iter().map(|r| r.item.clone()).collect();

    let context_pack = ContextPack {
        highlights: items.clone(),
        summaries: vec![],
        facts: vec![],
        citations: vec![],
    };

    tracing::debug!("Recalled {} items for query: {}", items.len(), req.query);

    Ok(Json(context_pack))
}
