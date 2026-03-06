use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use mom_core::{MemoryId, MemoryItem, Query, Scored, ScopeKey, MemoryStore};
use mom_service::ApiError;
use mom_store_surrealdb::SurrealDBStore;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

#[derive(Clone)]
struct AppState {
    store: Arc<SurrealDBStore>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mom=debug".parse()?),
        )
        .init();

    info!("🧠 MOM Service starting...");

    // Initialize SurrealDB store
    let db_path = std::env::var("MOM_DB_PATH")
        .unwrap_or_else(|_| "sqlite://mom.db".to_string());

    info!("Connecting to SurrealDB at {}", db_path);
    let store = SurrealDBStore::new(&db_path).await?;

    let state = AppState {
        store: Arc::new(store),
    };

    // Build router
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/memory", post(put_memory).get(list_memories))
        .route("/v1/memory/:id", get(get_memory).delete(delete_memory))
        .route("/v1/recall", post(recall))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await?;

    info!("✅ MOM API listening on http://{}", addr);
    info!("📚 Endpoints:");
    info!("  GET    /healthz              - Health check");
    info!("  POST   /v1/memory            - Write memory");
    info!("  GET    /v1/memory            - List memories");
    info!("  GET    /v1/memory/:id        - Get memory");
    info!("  DELETE /v1/memory/:id        - Delete memory");
    info!("  POST   /v1/recall            - Recall context");

    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn put_memory(
    State(st): State<AppState>,
    Json(mut item): Json<MemoryItem>,
) -> Result<(StatusCode, Json<MemoryItem>), ApiError> {
    // Generate ID if not provided
    if item.id.0.is_empty() {
        item.id = MemoryId(uuid::Uuid::new_v4().to_string());
    }

    st.store.put(item.clone()).await?;
    Ok((StatusCode::CREATED, Json(item)))
}

async fn get_memory(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MemoryItem>, ApiError> {
    let item = st
        .store
        .get(&MemoryId(id))
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(item))
}

async fn list_memories(
    State(st): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<MemoryItem>>, ApiError> {
    use mom_core::MemoryKind;

    // Parse scope parameters (required: tenant_id)
    let tenant_id = params
        .get("tenant_id")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "default".to_string());

    // Parse kinds filter (comma-separated, case-insensitive)
    let kinds = params.get("kinds").and_then(|s| {
        let kinds: Vec<_> = s
            .split(',')
            .filter_map(|k| {
                match k.trim().to_lowercase().as_str() {
                    "event" => Some(MemoryKind::Event),
                    "summary" => Some(MemoryKind::Summary),
                    "fact" => Some(MemoryKind::Fact),
                    "preference" => Some(MemoryKind::Preference),
                    _ => None,
                }
            })
            .collect();
        if kinds.is_empty() { None } else { Some(kinds) }
    });

    // Parse tags filter (comma-separated)
    let tags_any = params.get("tags").and_then(|s| {
        let tags: Vec<_> = s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if tags.is_empty() { None } else { Some(tags) }
    });

    // Parse time range (milliseconds since epoch)
    let since_ms = params
        .get("since_ms")
        .and_then(|s| s.parse::<i64>().ok());
    let until_ms = params
        .get("until_ms")
        .and_then(|s| s.parse::<i64>().ok());

    // Parse limit (default 10, max 100)
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .map(|l| l.min(100)) // Clamp to max 100
        .unwrap_or(10);

    let query = Query {
        scope: ScopeKey {
            tenant_id,
            workspace_id: params.get("workspace_id").cloned(),
            project_id: params.get("project_id").cloned(),
            agent_id: params.get("agent_id").cloned(),
            run_id: params.get("run_id").cloned(),
        },
        text: String::new(),
        kinds,
        tags_any,
        limit,
        since_ms,
        until_ms,
    };

    let results = st.store.query(query).await?;
    Ok(Json(results.into_iter().map(|s| s.item).collect()))
}

async fn delete_memory(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    st.store.delete(&MemoryId(id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn recall(
    State(st): State<AppState>,
    Json(mut q): Json<Query>,
) -> Result<Json<Vec<Scored<MemoryItem>>>, ApiError> {
    // Set default tenant if not provided
    if q.scope.tenant_id.is_empty() {
        q.scope.tenant_id = "default".to_string();
    }

    let results = st.store.query(q).await?;
    Ok(Json(results))
}
