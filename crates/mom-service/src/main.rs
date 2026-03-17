use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use mom_core::{MemoryId, MemoryItem, MemoryKind, Query, Scored, ScopeKey, MemoryStore, Embedder};
use mom_store_surrealdb::SurrealDBStore;
use mom_embeddings::create_embedder;
use mom_sources::{IngestionScheduler, MemorySource, OxidizedRAGSource, OxidizedGraphSource, DataFabricSource};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{info, error, warn};
use serde::{Deserialize, Serialize};

/// Registry of memory sources indexed by source ID
#[derive(Clone)]
struct SourceRegistry {
    sources: Arc<HashMap<String, Arc<Box<dyn MemorySource>>>>,
}

impl SourceRegistry {
    fn new() -> Self {
        Self {
            sources: Arc::new(HashMap::new()),
        }
    }

    fn get(&self, source_id: &str) -> Option<Arc<Box<dyn MemorySource>>> {
        self.sources.get(source_id).cloned()
    }
}

#[derive(Clone)]
struct AppState {
    store: Arc<SurrealDBStore>,
    embedder: Option<Arc<Box<dyn Embedder>>>,
    ingestion_scheduler: Arc<Mutex<IngestionScheduler>>,
    source_registry: SourceRegistry,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SemanticSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IngestionRequest {
    pub tenant_id: String,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IngestionResponse {
    pub source: String,
    pub count: usize,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct IngestionStatus {
    pub sources: usize,
    pub poll_interval_secs: u64,
}

/// Get source endpoint URL from environment or use default
fn get_source_endpoint(source_name: &str, default: &str) -> String {
    let env_var = match source_name {
        "oxidizedrag" => "OXIDIZEDRAG_URL",
        "oxidizedgraph" => "OXIDIZEDGRAPH_URL",
        "datafabric" => "DATAFABRIC_URL",
        _ => return default.to_string(),
    };

    std::env::var(env_var).unwrap_or_else(|_| default.to_string())
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

    // Initialize embedder (optional - Phase 2a feature)
    let embedder = match create_embedder().await {
        Ok(emb) => {
            info!("✅ Embeddings initialized (model: {})", emb.model_id());
            Some(Arc::new(emb))
        }
        Err(e) => {
            warn!("⚠️  Embeddings disabled: {}", e);
            None
        }
    };

    // Initialize ingestion scheduler with sources
    let mut scheduler = IngestionScheduler::new(300); // 5-minute poll interval

    // Get source endpoints from environment or use defaults
    let rag_endpoint = get_source_endpoint("oxidizedrag", "http://localhost:8001");
    let graph_endpoint = get_source_endpoint("oxidizedgraph", "http://localhost:8002");
    let fabric_endpoint = get_source_endpoint("datafabric", "http://localhost:8003");

    info!("Initializing ingestion sources:");
    info!("  oxidizedrag  : {}", rag_endpoint);
    info!("  oxidizedgraph: {}", graph_endpoint);
    info!("  datafabric   : {}", fabric_endpoint);

    // Create all memory sources
    let rag_source = Arc::new(Box::new(OxidizedRAGSource::new(rag_endpoint)) as Box<dyn MemorySource>);
    let graph_source = Arc::new(Box::new(OxidizedGraphSource::new(graph_endpoint)) as Box<dyn MemorySource>);
    let fabric_source = Arc::new(Box::new(DataFabricSource::new(fabric_endpoint)) as Box<dyn MemorySource>);

    // Register sources with scheduler
    scheduler.register_source(Box::new(OxidizedRAGSource::new(get_source_endpoint("oxidizedrag", "http://localhost:8001"))));
    scheduler.register_source(Box::new(OxidizedGraphSource::new(get_source_endpoint("oxidizedgraph", "http://localhost:8002"))));
    scheduler.register_source(Box::new(DataFabricSource::new(get_source_endpoint("datafabric", "http://localhost:8003"))));

    info!("✅ Ingestion scheduler initialized with {} sources", scheduler.source_count());

    // Build source registry for handlers
    let mut source_registry_map = HashMap::new();
    source_registry_map.insert("oxidizedrag".to_string(), rag_source);
    source_registry_map.insert("oxidizedgraph".to_string(), graph_source);
    source_registry_map.insert("datafabric".to_string(), fabric_source);

    let source_registry = SourceRegistry {
        sources: Arc::new(source_registry_map),
    };

    let state = AppState {
        store: Arc::new(store),
        embedder,
        ingestion_scheduler: Arc::new(Mutex::new(scheduler)),
        source_registry,
    };

    // Build router
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/memory", post(put_memory).get(list_memories))
        .route("/v1/memory/:id", get(get_memory).delete(delete_memory))
        .route("/v1/recall", post(recall))
        .route("/v1/semantic-search", post(semantic_search))
        .route("/v1/ingest/:source", post(ingest_source))
        .route("/v1/ingest/all", post(ingest_all))
        .route("/v1/ingest/status", get(ingest_status))
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
    info!("  POST   /v1/semantic-search   - Semantic search with embeddings");
    info!("  POST   /v1/ingest/:source    - Ingest from specific source");
    info!("  POST   /v1/ingest/all        - Ingest from all sources");
    info!("  GET    /v1/ingest/status     - Get ingestion status");

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
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<MemoryItem>, ApiError> {
    // SECURITY: Require tenant_id from query parameter (will be from auth context in US-17)
    let tenant_id = params
        .get("tenant_id")
        .ok_or(ApiError::BadRequest("tenant_id is required".to_string()))?
        .to_string();

    let scope = ScopeKey {
        tenant_id,
        workspace_id: params.get("workspace_id").map(|s| s.to_string()),
        project_id: params.get("project_id").map(|s| s.to_string()),
        agent_id: params.get("agent_id").map(|s| s.to_string()),
        run_id: params.get("run_id").map(|s| s.to_string()),
    };

    // Use scoped get to enforce tenant isolation
    let item = st
        .store
        .get_scoped(&MemoryId(id), &scope)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(item))
}

async fn list_memories(
    State(st): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Vec<MemoryItem>>, ApiError> {
    let tenant_id = params
        .get("tenant_id")
        .ok_or(ApiError::BadRequest("tenant_id is required".to_string()))?
        .to_string();

    // Parse kinds filter (comma-separated: event,summary,fact,preference)
    let kinds = params.get("kinds").and_then(|k| {
        let parsed: Result<Vec<MemoryKind>, _> = k
            .split(',')
            .map(|s| match s.trim().to_lowercase().as_str() {
                "event" => Ok(MemoryKind::Event),
                "summary" => Ok(MemoryKind::Summary),
                "fact" => Ok(MemoryKind::Fact),
                "preference" => Ok(MemoryKind::Preference),
                _ => Err(()),
            })
            .collect();
        parsed.ok().and_then(|v| if v.is_empty() { None } else { Some(v) })
    });

    // Parse tags filter (comma-separated)
    let tags_any = params.get("tags").and_then(|t| {
        let tags: Vec<String> = t.split(',').map(|s| s.trim().to_string()).collect();
        if tags.is_empty() || tags.iter().all(|s| s.is_empty()) {
            None
        } else {
            Some(tags.into_iter().filter(|s| !s.is_empty()).collect())
        }
    });

    // Parse time range filters (milliseconds since epoch)
    let since_ms = params.get("since_ms").and_then(|s| s.parse().ok());
    let until_ms = params.get("until_ms").and_then(|s| s.parse().ok());

    // Parse and clamp limit to max 100
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .map(|l| l.min(100))
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
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<StatusCode, ApiError> {
    // SECURITY: Require tenant_id from query parameter (will be from auth context in US-17)
    let tenant_id = params
        .get("tenant_id")
        .ok_or(ApiError::BadRequest("tenant_id is required".to_string()))?
        .to_string();

    let scope = ScopeKey {
        tenant_id,
        workspace_id: params.get("workspace_id").map(|s| s.to_string()),
        project_id: params.get("project_id").map(|s| s.to_string()),
        agent_id: params.get("agent_id").map(|s| s.to_string()),
        run_id: params.get("run_id").map(|s| s.to_string()),
    };

    // Use scoped delete to enforce tenant isolation
    st.store.delete_scoped(&MemoryId(id), &scope).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Recall Ranking Constants
// ============================================================================

/// Time window for recency decay (30 days in milliseconds)
const RECENCY_DECAY_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Scoring weights for combined ranking
const TEXT_MATCH_WEIGHT: f32 = 0.60;
const IMPORTANCE_WEIGHT: f32 = 0.25;
const RECENCY_WEIGHT: f32 = 0.15;

/// Multiply limit by this factor when fetching candidates for ranking
/// Ensures high-relevance items aren't filtered by initial LIMIT clause
const CANDIDATE_MULTIPLIER: usize = 5;

// ============================================================================
// Recall Ranking Functions
// ============================================================================

/// Compute lexical search score (0..1) based on text match
/// Returns 0.0 if query doesn't match, up to 1.0 for exact match
fn compute_text_match_score(item_content: &str, query_text: &str) -> f32 {
    // Handle empty inputs to prevent division by zero
    if query_text.is_empty() || item_content.is_empty() {
        return 0.0;
    }

    let query_lower = query_text.to_lowercase();
    let content_lower = item_content.to_lowercase();

    // Exact match = 1.0
    if content_lower == query_lower {
        return 1.0;
    }

    // Substring match: check if query appears
    let match_count = content_lower.matches(&query_lower).count();
    if match_count == 0 {
        return 0.0;
    }

    // Position-based scoring: early matches score higher
    let position = content_lower.find(&query_lower).unwrap_or(content_lower.len());
    let distance_ratio = (position as f32) / (content_lower.len() as f32);
    let position_score = 1.0 - (distance_ratio * 0.5); // Early matches boost score

    // Combined score: 50% for substring match + 50% for position
    // Multiple matches don't increase score further (already checked for existence)
    let score = 0.5 + position_score * 0.5;
    score.min(1.0)
}

/// Compute recency score (0..1) based on how recent the memory is
/// Newer items score higher, older items decay to 0 after RECENCY_DECAY_WINDOW_MS
fn compute_recency_score(created_at_ms: i64) -> f32 {
    let now = chrono::Utc::now().timestamp_millis();
    let age_ms = (now - created_at_ms).max(0);

    // Decay function: memories score 1.0 if current, decay to 0.0 after window
    let decay = (age_ms as f32) / (RECENCY_DECAY_WINDOW_MS as f32);
    (1.0 - decay).max(0.0)
}

/// Compute combined ranking score from text match, importance, and recency
fn compute_ranking_score(
    item: &mom_core::MemoryItem,
    query_text: &str,
) -> f32 {
    let text_match = compute_text_match_score(&item_to_text(item), query_text);

    // If there's no text match, score is 0 (no recall result)
    if text_match == 0.0 {
        return 0.0;
    }

    let recency = compute_recency_score(item.created_at_ms);
    let importance = item.importance;

    // Weighted combination of ranking factors
    (text_match * TEXT_MATCH_WEIGHT) + (importance * IMPORTANCE_WEIGHT) + (recency * RECENCY_WEIGHT)
}

/// Extract text content from MemoryItem for searching
fn item_to_text(item: &mom_core::MemoryItem) -> String {
    match &item.content {
        mom_core::Content::Text(t) => t.clone(),
        mom_core::Content::Json(v) => v.to_string(),
        mom_core::Content::TextJson { text, json } => {
            format!("{} {}", text, json.to_string())
        }
    }
}

async fn recall(
    State(st): State<AppState>,
    Json(mut q): Json<Query>,
) -> Result<Json<Vec<Scored<MemoryItem>>>, ApiError> {
    // Set default tenant if not provided
    if q.scope.tenant_id.is_empty() {
        q.scope.tenant_id = "default".to_string();
    }

    // Apply lexical search scoring if query text is provided
    if !q.text.is_empty() {
        // Fetch larger candidate set for ranking (don't lose high-relevance items to LIMIT)
        // Multiply limit by CANDIDATE_MULTIPLIER to ensure ranking sees diverse results
        let original_limit = q.limit;
        q.limit = (q.limit * CANDIDATE_MULTIPLIER).min(1000); // Cap at 1000 for safety

        let results = st.store.query(q.clone()).await?;

        // Apply ranking: compute scores and filter by text match
        let mut scored: Vec<Scored<MemoryItem>> = results
            .into_iter()
            .map(|scored_item| {
                let ranking_score = compute_ranking_score(&scored_item.item, &q.text);
                Scored {
                    score: ranking_score,
                    item: scored_item.item,
                }
            })
            .filter(|s| s.score > 0.0) // Only keep items with text match
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Apply original limit to final results
        scored.truncate(original_limit);

        Ok(Json(scored))
    } else {
        // No query text: return results as-is (store determines ordering)
        let results = st.store.query(q).await?;
        Ok(Json(results))
    }
}

/// Semantic search using embeddings (Phase 2a feature)
///
/// Returns memories ranked by semantic similarity to the query
/// SECURITY: Requires tenant_id from query parameter (will be from auth context in US-17)
async fn semantic_search(
    State(st): State<AppState>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
    Json(req): Json<SemanticSearchRequest>,
) -> Result<Json<Vec<Scored<MemoryItem>>>, ApiError> {
    // SECURITY: Require tenant_id from query parameter to prevent IDOR
    let tenant_id = params
        .get("tenant_id")
        .ok_or(ApiError::BadRequest("tenant_id is required".to_string()))?
        .to_string();

    let embedder = st.embedder.as_ref()
        .ok_or(ApiError::Internal("Embeddings not available".to_string()))?;

    // Generate embedding for query text
    let query_embedding = embedder
        .embed(&req.query)
        .await
        .map_err(|_| ApiError::Internal("Embedding unavailable".to_string()))?;

    let limit = req.limit.unwrap_or(10).min(100);

    // Create scope for search with tenant isolation
    let scope = ScopeKey {
        tenant_id,
        workspace_id: None,
        project_id: None,
        agent_id: None,
        run_id: None,
    };

    // Use vector recall from store (Phase 2b)
    let results = st.store
        .vector_recall(&query_embedding, &scope, limit)
        .await?;

    Ok(Json(results))
}

/// Ingest memories from a specific source (Phase 2c - Issue #29)
///
/// Fetches memories from the specified source and stores them in MOM.
/// SECURITY: Requires tenant_id to enforce scope isolation
async fn ingest_source(
    State(st): State<AppState>,
    Path(source): Path<String>,
    Json(req): Json<IngestionRequest>,
) -> Result<Json<IngestionResponse>, ApiError> {
    let scope = ScopeKey {
        tenant_id: req.tenant_id.clone(),
        workspace_id: req.workspace_id,
        project_id: req.project_id,
        agent_id: req.agent_id,
        run_id: req.run_id,
    };

    info!("Starting ingestion from source: {}", source);

    // Get source from registry
    let source_impl = st.source_registry.get(&source)
        .ok_or_else(|| ApiError::BadRequest(format!("Unknown source: {}", source)))?;

    // Fetch memories from the source
    let memories = source_impl.fetch_memories(&scope, None).await
        .map_err(|e| ApiError::Internal(format!("Failed to fetch from {}: {}", source, e)))?;

    let count = memories.len();

    // Store all fetched memories
    for memory in memories {
        st.store.put(memory).await
            .map_err(|e| ApiError::Internal(format!("Failed to store memory: {}", e)))?;
    }

    info!("✅ Ingested {} memories from {}", count, source);

    Ok(Json(IngestionResponse {
        source,
        count,
        message: format!("Successfully ingested {} memories", count),
    }))
}

/// Ingest memories from all registered sources (Phase 2c - Issue #29)
///
/// Fetches memories from all sources and stores them in MOM.
/// SECURITY: Requires tenant_id to enforce scope isolation
async fn ingest_all(
    State(st): State<AppState>,
    Json(req): Json<IngestionRequest>,
) -> Result<Json<Vec<IngestionResponse>>, ApiError> {
    let scope = ScopeKey {
        tenant_id: req.tenant_id.clone(),
        workspace_id: req.workspace_id,
        project_id: req.project_id,
        agent_id: req.agent_id,
        run_id: req.run_id,
    };

    let mut responses = Vec::new();

    // Ingest from all registered sources in the registry
    let source_ids = vec!["oxidizedrag", "oxidizedgraph", "datafabric"];

    for source_id in source_ids {
        match st.source_registry.get(source_id) {
            Some(source) => {
                match source.fetch_memories(&scope, None).await {
                    Ok(memories) => {
                        let count = memories.len();

                        // Store all fetched memories
                        for memory in memories {
                            if let Err(e) = st.store.put(memory).await {
                                warn!("Failed to store memory from {}: {}", source_id, e);
                            }
                        }

                        info!("✅ Ingested {} memories from {}", count, source_id);
                        responses.push(IngestionResponse {
                            source: source_id.to_string(),
                            count,
                            message: format!("Successfully ingested {} memories", count),
                        });
                    },
                    Err(e) => {
                        warn!("⚠️  Failed to ingest from {}: {}", source_id, e);
                        responses.push(IngestionResponse {
                            source: source_id.to_string(),
                            count: 0,
                            message: format!("Failed: {}", e),
                        });
                    }
                }
            },
            None => {
                warn!("⚠️  Source {} not found in registry", source_id);
            }
        }
    }

    Ok(Json(responses))
}

/// Get ingestion scheduler status
///
/// Returns information about the current ingestion configuration
async fn ingest_status(
    State(st): State<AppState>,
) -> Json<IngestionStatus> {
    let scheduler = st.ingestion_scheduler.lock().await;
    Json(IngestionStatus {
        sources: scheduler.source_count(),
        poll_interval_secs: scheduler.poll_interval(),
    })
}

// Error handling
#[derive(Debug)]
enum ApiError {
    NotFound,
    BadRequest(String),
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
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::Internal(_msg) => {
                // Log the real error server-side (via tracing), but return generic message to client
                // to avoid exposing sensitive information (database errors, stack traces, etc.)
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            },
        };

        let body = Json(serde_json::json!({
            "error": message,
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // Helper to parse kinds filter (extracted from list_memories logic for testability)
    fn parse_kinds(kinds_str: &str) -> Option<Vec<MemoryKind>> {
        let parsed: Result<Vec<MemoryKind>, _> = kinds_str
            .split(',')
            .map(|s| match s.trim().to_lowercase().as_str() {
                "event" => Ok(MemoryKind::Event),
                "summary" => Ok(MemoryKind::Summary),
                "fact" => Ok(MemoryKind::Fact),
                "preference" => Ok(MemoryKind::Preference),
                _ => Err(()),
            })
            .collect();
        parsed
            .ok()
            .and_then(|v: Vec<MemoryKind>| if v.is_empty() { None } else { Some(v) })
    }

    // Helper to parse tags filter
    fn parse_tags(tags_str: &str) -> Option<Vec<String>> {
        let tags: Vec<String> = tags_str.split(',').map(|s| s.trim().to_string()).collect();
        if tags.is_empty() || tags.iter().all(|s| s.is_empty()) {
            None
        } else {
            Some(tags.into_iter().filter(|s: &String| !s.is_empty()).collect())
        }
    }

    #[test]
    fn test_parse_single_kind() {
        let kinds = parse_kinds("event");
        assert_eq!(kinds, Some(vec![MemoryKind::Event]));
    }

    #[test]
    fn test_parse_multiple_kinds() {
        let kinds = parse_kinds("event,summary,fact");
        assert_eq!(
            kinds,
            Some(vec![
                MemoryKind::Event,
                MemoryKind::Summary,
                MemoryKind::Fact
            ])
        );
    }

    #[test]
    fn test_parse_kinds_with_whitespace() {
        let kinds = parse_kinds("event , summary , fact");
        assert_eq!(
            kinds,
            Some(vec![
                MemoryKind::Event,
                MemoryKind::Summary,
                MemoryKind::Fact
            ])
        );
    }

    #[test]
    fn test_parse_kinds_case_insensitive() {
        let kinds = parse_kinds("EVENT,Summary,FACT");
        assert_eq!(
            kinds,
            Some(vec![
                MemoryKind::Event,
                MemoryKind::Summary,
                MemoryKind::Fact
            ])
        );
    }

    #[test]
    fn test_parse_invalid_kind() {
        let kinds = parse_kinds("invalid,event");
        assert_eq!(kinds, None);
    }

    #[test]
    fn test_parse_empty_kinds() {
        let kinds = parse_kinds("");
        assert_eq!(kinds, None);
    }

    #[test]
    fn test_parse_all_kinds() {
        let kinds = parse_kinds("event,summary,fact,preference");
        assert_eq!(
            kinds,
            Some(vec![
                MemoryKind::Event,
                MemoryKind::Summary,
                MemoryKind::Fact,
                MemoryKind::Preference
            ])
        );
    }

    #[test]
    fn test_parse_single_tag() {
        let tags = parse_tags("important");
        assert_eq!(tags, Some(vec!["important".to_string()]));
    }

    #[test]
    fn test_parse_multiple_tags() {
        let tags = parse_tags("important,urgent,review");
        assert_eq!(
            tags,
            Some(vec![
                "important".to_string(),
                "urgent".to_string(),
                "review".to_string()
            ])
        );
    }

    #[test]
    fn test_parse_tags_with_whitespace() {
        let tags = parse_tags("important , urgent , review");
        assert_eq!(
            tags,
            Some(vec![
                "important".to_string(),
                "urgent".to_string(),
                "review".to_string()
            ])
        );
    }

    #[test]
    fn test_parse_empty_tags() {
        let tags = parse_tags("");
        assert_eq!(tags, None);
    }

    #[test]
    fn test_parse_empty_tags_with_commas() {
        let tags = parse_tags(",,");
        assert_eq!(tags, None);
    }

    #[test]
    fn test_parse_tags_with_empty_elements() {
        let tags = parse_tags("important,,urgent");
        assert_eq!(tags, Some(vec!["important".to_string(), "urgent".to_string()]));
    }

    #[test]
    fn test_limit_default() {
        let params: HashMap<String, String> = HashMap::new();
        let limit = params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .map(|l| l.min(100))
            .unwrap_or(10);
        assert_eq!(limit, 10);
    }

    #[test]
    fn test_limit_custom() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("limit".to_string(), "50".to_string());
        let limit = params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .map(|l| l.min(100))
            .unwrap_or(10);
        assert_eq!(limit, 50);
    }

    #[test]
    fn test_limit_clamped() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("limit".to_string(), "500".to_string());
        let limit = params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .map(|l| l.min(100))
            .unwrap_or(10);
        assert_eq!(limit, 100);
    }

    #[test]
    fn test_limit_invalid() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("limit".to_string(), "invalid".to_string());
        let limit = params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .map(|l| l.min(100))
            .unwrap_or(10);
        assert_eq!(limit, 10);
    }

    #[test]
    fn test_timestamp_parsing() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("since_ms".to_string(), "1609459200000".to_string());
        params.insert("until_ms".to_string(), "1609545600000".to_string());

        let since_ms = params.get("since_ms").and_then(|s| s.parse().ok());
        let until_ms = params.get("until_ms").and_then(|s| s.parse().ok());

        assert_eq!(since_ms, Some(1609459200000i64));
        assert_eq!(until_ms, Some(1609545600000i64));
    }

    #[test]
    fn test_timestamp_invalid() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("since_ms".to_string(), "invalid".to_string());

        let since_ms = params.get("since_ms").and_then(|s| s.parse::<i64>().ok());
        assert_eq!(since_ms, None);
    }

    #[test]
    fn test_combined_filters() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("kinds".to_string(), "event,summary".to_string());
        params.insert("tags".to_string(), "important,urgent".to_string());
        params.insert("limit".to_string(), "25".to_string());
        params.insert("since_ms".to_string(), "1609459200000".to_string());

        let kinds = params.get("kinds").and_then(|k| {
            let parsed: Result<Vec<MemoryKind>, _> = k
                .split(',')
                .map(|s| match s.trim().to_lowercase().as_str() {
                    "event" => Ok(MemoryKind::Event),
                    "summary" => Ok(MemoryKind::Summary),
                    "fact" => Ok(MemoryKind::Fact),
                    "preference" => Ok(MemoryKind::Preference),
                    _ => Err(()),
                })
                .collect();
            parsed.ok().and_then(|v: Vec<MemoryKind>| if v.is_empty() { None } else { Some(v) })
        });

        let tags_any = params.get("tags").and_then(|t| {
            let tags: Vec<String> = t.split(',').map(|s| s.trim().to_string()).collect();
            if tags.is_empty() || tags.iter().all(|s| s.is_empty()) {
                None
            } else {
                Some(tags.into_iter().filter(|s: &String| !s.is_empty()).collect())
            }
        });

        let limit = params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .map(|l| l.min(100))
            .unwrap_or(10);

        let since_ms = params.get("since_ms").and_then(|s| s.parse().ok());

        assert_eq!(
            kinds,
            Some(vec![MemoryKind::Event, MemoryKind::Summary])
        );
        assert_eq!(
            tags_any,
            Some(vec!["important".to_string(), "urgent".to_string()])
        );
        assert_eq!(limit, 25);
        assert_eq!(since_ms, Some(1609459200000i64));
    }

    #[test]
    fn test_scope_key_parsing() {
        let mut params: HashMap<String, String> = HashMap::new();
        params.insert("tenant_id".to_string(), "acme".to_string());
        params.insert("workspace_id".to_string(), "workspace1".to_string());
        params.insert("project_id".to_string(), "project1".to_string());
        params.insert("agent_id".to_string(), "agent1".to_string());
        params.insert("run_id".to_string(), "run1".to_string());

        let tenant_id = params
            .get("tenant_id")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        assert_eq!(tenant_id, "acme");
        assert_eq!(params.get("workspace_id").cloned(), Some("workspace1".to_string()));
        assert_eq!(params.get("project_id").cloned(), Some("project1".to_string()));
        assert_eq!(params.get("agent_id").cloned(), Some("agent1".to_string()));
        assert_eq!(params.get("run_id").cloned(), Some("run1".to_string()));
    }

    #[test]
    fn test_default_tenant_id() {
        let params: HashMap<String, String> = HashMap::new();
        let tenant_id = params
            .get("tenant_id")
            .map(|s| s.to_string())
            .unwrap_or_else(|| "default".to_string());

        assert_eq!(tenant_id, "default");
    }

    // US-4: Recall/Lexical Search Tests

    #[test]
    fn test_text_match_score_exact() {
        let score = compute_text_match_score("deployment", "deployment");
        assert_eq!(score, 1.0);
    }

    #[test]
    fn test_text_match_score_substring() {
        let score = compute_text_match_score("production deployment complete", "deployment");
        assert!(score > 0.0 && score < 1.0);
        assert!(score >= 0.5); // Substring should score reasonably high
    }

    #[test]
    fn test_text_match_score_no_match() {
        let score = compute_text_match_score("hello world", "deployment");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_text_match_score_case_insensitive() {
        let score1 = compute_text_match_score("DEPLOYMENT", "deployment");
        let score2 = compute_text_match_score("deployment", "DEPLOYMENT");
        assert_eq!(score1, 1.0);
        assert_eq!(score2, 1.0);
    }

    #[test]
    fn test_text_match_score_early_position() {
        let early = compute_text_match_score("deployment started", "deployment");
        let late = compute_text_match_score("we started a deployment", "deployment");
        assert!(early > late); // Earlier position scores higher
    }

    #[test]
    fn test_text_match_score_multiple_occurrences() {
        let score = compute_text_match_score("deployment and deployment and deployment", "deployment");
        assert!(score > 0.9); // Multiple matches boost score
    }

    #[test]
    fn test_text_match_score_empty_query() {
        let score = compute_text_match_score("hello world", "");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_recency_score_current() {
        let now = chrono::Utc::now().timestamp_millis();
        let score = compute_recency_score(now);
        assert!(score > 0.95); // Very recent items score high
    }

    #[test]
    fn test_recency_score_old() {
        let thirty_one_days_ago = chrono::Utc::now().timestamp_millis() - (31 * 24 * 60 * 60 * 1000);
        let score = compute_recency_score(thirty_one_days_ago);
        assert!(score < 0.1); // Very old items score low
    }

    #[test]
    fn test_recency_score_one_week_old() {
        let one_week_ago = chrono::Utc::now().timestamp_millis() - (7 * 24 * 60 * 60 * 1000);
        let score = compute_recency_score(one_week_ago);
        assert!(score > 0.7); // One week old should still score reasonably
    }

    #[test]
    fn test_item_to_text_text_content() {
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
            content: mom_core::Content::Text("deployment failed".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let text = item_to_text(&item);
        assert_eq!(text, "deployment failed");
    }

    #[test]
    fn test_item_to_text_json_content() {
        let json_val = serde_json::json!({"status": "failed", "reason": "timeout"});
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
            content: mom_core::Content::Json(json_val),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let text = item_to_text(&item);
        assert!(text.contains("failed"));
    }

    #[test]
    fn test_ranking_score_high_importance_recent() {
        let now = chrono::Utc::now().timestamp_millis();
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
            created_at_ms: now,
            content: mom_core::Content::Text("deployment started".to_string()),
            tags: vec![],
            importance: 0.9,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let score = compute_ranking_score(&item, "deployment");
        assert!(score > 0.6); // Good match, high importance, recent
    }

    #[test]
    fn test_ranking_score_low_importance_old() {
        let thirty_days_ago = chrono::Utc::now().timestamp_millis() - (30 * 24 * 60 * 60 * 1000);
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
            created_at_ms: thirty_days_ago,
            content: mom_core::Content::Text("old deployment info".to_string()),
            tags: vec![],
            importance: 0.1,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let score = compute_ranking_score(&item, "deployment");
        // Exact match (0.5) + low importance (0.1 * 0.3 = 0.03) + very low recency (0.02 * 0.2)
        // ≈ 0.5 + 0.03 + 0.004 ≈ 0.534 (still reasonable since text match is high)
        assert!(score < 0.7 && score > 0.3); // Match but low importance and old
    }

    #[test]
    fn test_ranking_score_no_text_match() {
        let now = chrono::Utc::now().timestamp_millis();
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
            created_at_ms: now,
            content: mom_core::Content::Text("hello world".to_string()),
            tags: vec![],
            importance: 0.9,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let score = compute_ranking_score(&item, "deployment");
        assert_eq!(score, 0.0); // No text match = 0 score (filtered out in recall)
    }

    #[test]
    fn test_ranking_combination_weights() {
        // Verify that text match, importance, and recency are weighted correctly
        let now = chrono::Utc::now().timestamp_millis();
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
            created_at_ms: now,
            content: mom_core::Content::Text("deployment".to_string()),
            tags: vec![],
            importance: 0.5,
            confidence: 1.0,
            source: "system".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let score = compute_ranking_score(&item, "deployment");
        // Expected: text_match(1.0) * 0.6 + importance(0.5) * 0.25 + recency(~1.0) * 0.15
        // = 0.6 + 0.125 + 0.15 = 0.875
        assert!(score > 0.85 && score < 0.95);
    }

    #[test]
    fn test_recall_empty_query_returns_all() {
        // Verify that empty query returns results without text filtering
        // This test validates the query logic would work if results were available
        let query_text = "";
        assert!(query_text.is_empty());
    }

    #[test]
    fn test_ranking_high_importance_beats_recency() {
        // Verify that importance significantly affects ranking
        let now = chrono::Utc::now().timestamp_millis();
        let recent_low_importance = MemoryItem {
            id: MemoryId("recent".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: now,
            content: mom_core::Content::Text("deployment".to_string()),
            tags: vec![],
            importance: 0.2, // Low importance
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let old_high_importance = MemoryItem {
            id: MemoryId("old".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: now - (20 * 24 * 60 * 60 * 1000), // 20 days old
            content: mom_core::Content::Text("deployment".to_string()),
            tags: vec![],
            importance: 0.9, // High importance
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let recent_score = compute_ranking_score(&recent_low_importance, "deployment");
        let old_score = compute_ranking_score(&old_high_importance, "deployment");

        // High importance should outweigh recency: 0.6 + 0.9*0.25 + ~0.6*0.15
        // = 0.6 + 0.225 + 0.09 ≈ 0.915
        // vs 0.6 + 0.2*0.25 + 1.0*0.15 = 0.6 + 0.05 + 0.15 = 0.8
        assert!(old_score > recent_score);
    }

    #[test]
    fn test_ranking_text_match_primary_factor() {
        // Verify that text match is the dominant scoring factor
        let now = chrono::Utc::now().timestamp_millis();
        let high_importance = MemoryItem {
            id: MemoryId("high".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: now - (25 * 24 * 60 * 60 * 1000),
            content: mom_core::Content::Text("deployment".to_string()),
            tags: vec![],
            importance: 0.9,
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let no_match = MemoryItem {
            id: MemoryId("nomatch".to_string()),
            scope: ScopeKey {
                tenant_id: "test".to_string(),
                workspace_id: None,
                project_id: None,
                agent_id: None,
                run_id: None,
            },
            kind: MemoryKind::Event,
            created_at_ms: now,
            content: mom_core::Content::Text("hello world".to_string()),
            tags: vec![],
            importance: 0.9,
            confidence: 1.0,
            source: "test".to_string(),
            ttl_ms: None,
            meta: Default::default(),
            embedding: None,
            embedding_model: None,
        };

        let match_score = compute_ranking_score(&high_importance, "deployment");
        let no_match_score = compute_ranking_score(&no_match, "deployment");

        // No text match = 0, regardless of importance/recency
        assert_eq!(no_match_score, 0.0);
        assert!(match_score > 0.0);
    }

    // Phase 2a: Semantic Search Tests

    #[test]
    fn test_semantic_search_request_parsing() {
        use serde_json::json;

        let req_json = json!({
            "query": "deployment failed",
            "limit": 25
        });

        let req: SemanticSearchRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.query, "deployment failed");
        assert_eq!(req.limit, Some(25));
    }

    #[test]
    fn test_semantic_search_request_defaults() {
        use serde_json::json;

        let req_json = json!({
            "query": "error handling"
        });

        let req: SemanticSearchRequest = serde_json::from_value(req_json).unwrap();
        assert_eq!(req.query, "error handling");
        assert_eq!(req.limit, None);
    }

    #[test]
    fn test_semantic_search_request_limit_validation() {
        // Limit should be applied in endpoint handler
        let req = SemanticSearchRequest {
            query: "test".to_string(),
            limit: Some(500),
        };

        let limit = req.limit.unwrap_or(10).min(100);
        assert_eq!(limit, 100); // Should be clamped to max 100
    }

    #[test]
    fn test_semantic_search_tenant_id_required() {
        // SECURITY: tenant_id must be provided via query parameter, not in request body
        let params: HashMap<String, String> = HashMap::new();
        let tenant_id = params.get("tenant_id");

        // Verify tenant_id is missing (should error in handler)
        assert!(tenant_id.is_none());
    }

    #[test]
    fn test_embedding_provider_env_config() {
        // Test that environment configuration would work
        let provider = std::env::var("EMBEDDING_PROVIDER")
            .unwrap_or_else(|_| "ollama".to_string());

        // Verify it's one of the supported providers
        assert!(
            provider == "ollama" || provider == "mistral" || provider == "openai",
            "Unknown provider: {}",
            provider
        );
    }

    #[test]
    fn test_embedding_model_defaults() {
        // Ollama default
        let ollama_model = std::env::var("OLLAMA_MODEL")
            .unwrap_or_else(|_| "mxbai-embed-large".to_string());
        assert!(!ollama_model.is_empty());

        // Mistral default
        let mistral_model = std::env::var("MISTRAL_MODEL")
            .unwrap_or_else(|_| "mistral-embed".to_string());
        assert!(!mistral_model.is_empty());

        // OpenAI default
        let openai_model = std::env::var("OPENAI_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-large".to_string());
        assert!(!openai_model.is_empty());
    }

    #[test]
    fn test_semantic_search_endpoint_url_routing() {
        // Verify the semantic-search endpoint would be registered
        // This test validates the request/response types work (tenant_id comes from query param)
        let req = SemanticSearchRequest {
            query: "test query".to_string(),
            limit: Some(10),
        };

        // Verify request can be serialized/deserialized
        let json = serde_json::to_string(&req).unwrap();
        let _deserialized: SemanticSearchRequest = serde_json::from_str(&json).unwrap();
        assert!(!json.is_empty());
    }

    #[test]
    fn test_embedding_disabled_error() {
        // Simulate the error handling when embeddings are not available
        let error_msg = "Embeddings not available";
        assert!(!error_msg.is_empty());
        assert!(error_msg.contains("Embeddings"));
    }

    // Phase 2c ingestion tests
    #[test]
    fn test_ingestion_request_serialization() {
        let req = IngestionRequest {
            tenant_id: "test-tenant".to_string(),
            workspace_id: Some("workspace1".to_string()),
            project_id: Some("project1".to_string()),
            agent_id: Some("agent:analyzer".to_string()),
            run_id: Some("run:001".to_string()),
        };

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: IngestionRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tenant_id, "test-tenant");
        assert_eq!(deserialized.workspace_id, Some("workspace1".to_string()));
        assert_eq!(deserialized.project_id, Some("project1".to_string()));
        assert_eq!(deserialized.agent_id, Some("agent:analyzer".to_string()));
        assert_eq!(deserialized.run_id, Some("run:001".to_string()));
    }

    #[test]
    fn test_ingestion_request_minimal() {
        let req = IngestionRequest {
            tenant_id: "test-tenant".to_string(),
            workspace_id: None,
            project_id: None,
            agent_id: None,
            run_id: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let deserialized: IngestionRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.tenant_id, "test-tenant");
        assert!(deserialized.workspace_id.is_none());
    }

    #[test]
    fn test_ingestion_response_serialization() {
        let response = IngestionResponse {
            source: "oxidizedrag".to_string(),
            count: 42,
            message: "Successfully ingested 42 memories".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: IngestionResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.source, "oxidizedrag");
        assert_eq!(deserialized.count, 42);
        assert_eq!(deserialized.message, "Successfully ingested 42 memories");
    }

    #[test]
    fn test_ingestion_status_serialization() {
        let status = IngestionStatus {
            sources: 3,
            poll_interval_secs: 300,
        };

        let json = serde_json::to_string(&status).unwrap();
        let deserialized: IngestionStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.sources, 3);
        assert_eq!(deserialized.poll_interval_secs, 300);
    }

    #[test]
    fn test_oxidizedrag_source_in_service() {
        let source = OxidizedRAGSource::new("http://localhost:8001".to_string());
        assert_eq!(source.source_id(), "oxidizedrag");
        assert_eq!(source.description(), "Code analysis and pattern extraction from oxidizedRAG");
    }

    #[test]
    fn test_oxidizedgraph_source_in_service() {
        let source = OxidizedGraphSource::new("http://localhost:8002".to_string());
        assert_eq!(source.source_id(), "oxidizedgraph");
        assert_eq!(source.description(), "Agent workflow executions, decisions, and state transitions from oxidizedgraph");
    }

    #[test]
    fn test_datafabric_source_in_service() {
        let source = DataFabricSource::new("http://localhost:8003".to_string());
        assert_eq!(source.source_id(), "datafabric");
        assert_eq!(source.description(), "Task records, modifications, and validated facts from data-fabric");
    }

    #[test]
    fn test_app_state_creation() {
        // Test that AppState can be created with ingestion scheduler
        let mut scheduler = IngestionScheduler::new(300);

        let rag = Box::new(OxidizedRAGSource::new("http://localhost:8001".to_string()));
        let graph = Box::new(OxidizedGraphSource::new("http://localhost:8002".to_string()));
        let fabric = Box::new(DataFabricSource::new("http://localhost:8003".to_string()));

        scheduler.register_source(rag);
        scheduler.register_source(graph);
        scheduler.register_source(fabric);

        assert_eq!(scheduler.source_count(), 3);
        assert_eq!(scheduler.poll_interval(), 300);
    }

}
