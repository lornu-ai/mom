# MOM Phase 2: Vector Embeddings & Hybrid Search Design

**Status**: In Progress
**Target Issues**: #10, #12
**Related**: Issue #3 (Architecture), Issue #7 (Recall)

## Overview

Phase 2 adds semantic search to MOM's existing lexical retrieval. This document details the design for vector embeddings + hybrid recall.

## Architecture: Embedding Provider

### Embedder Trait (Already Stubbed in Phase 1)

Located in `mom-core/src/lib.rs`:

```rust
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, input: &str) -> anyhow::Result<Vec<f32>>;
    fn dims(&self) -> usize;
    fn model_id(&self) -> &str;
}
```

### Supported Providers (Phase 2)

1. **Ollama** (local, self-hosted)
   - Embedding models: `mxbai-embed-large`, `nomic-embed-text`
   - Free, offline, runs on CPU/GPU
   - Recommended for development & deployment

2. **Mistral** (API-based)
   - Model: `mistral-embed`
   - Fast, high quality, requires API key
   - Recommended for production

3. **OpenAI** (API-based)
   - Model: `text-embedding-3-large` or `text-embedding-3-small`
   - High quality, requires API key

### Implementation Plan

**Crate**: `mom-embeddings` (new)

```
mom/
  crates/
    mom-core/
    mom-store-surrealdb/
    mom-service/
    mom-embeddings/           # NEW
      src/
        lib.rs
        ollama.rs            # Ollama implementation
        mistral.rs           # Mistral implementation
        openai.rs            # OpenAI implementation
        batch.rs             # Batch embedding helper
```

## Data Model Changes

### 1. Add `embedding` Field to MemoryItem

**File**: `crates/mom-core/src/lib.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: MemoryId,
    pub scope: ScopeKey,
    pub kind: MemoryKind,
    pub created_at_ms: i64,
    pub content: Content,
    pub tags: Vec<String>,

    pub importance: f32,   // 0..1
    pub confidence: f32,   // 0..1

    pub source: String,
    pub ttl_ms: Option<i64>,
    pub meta: BTreeMap<String, serde_json::Value>,

    // NEW: Vector embedding for semantic search
    pub embedding: Option<Vec<f32>>,      // Optional during creation
    pub embedding_model: Option<String>,  // Track which model generated it
}
```

### 2. Update SurrealDB Schema

**File**: `crates/mom-store-surrealdb/src/lib.rs`

Add to `init_schema()`:

```sql
DEFINE FIELD embedding ON TABLE memory_items TYPE option<array<float>>;
DEFINE FIELD embedding_model ON TABLE memory_items TYPE option<string>;

-- Vector index (dimensions depend on embedding model)
-- For mxbai-embed-large: 1024 dims
-- For mistral-embed: 1024 dims
-- For text-embedding-3-large: 3072 dims
DEFINE INDEX idx_embedding ON TABLE memory_items COLUMNS embedding;
```

### 3. Update StoredItem Structure

```rust
#[derive(Debug, Serialize, Deserialize, Clone)]
struct StoredItem {
    // ... existing fields ...
    embedding: Option<Vec<f32>>,
    embedding_model: Option<String>,
}
```

## Embedding Workflow

### 1. On Write (PUT)

```
User writes MemoryItem
  ↓
MemoryStore.put() receives item
  ↓
If embedding is None AND content is non-empty:
  ├─ Call embedder.embed(content_text)
  └─ Store result in embedding field
  ↓
Insert item + embedding into SurrealDB
```

### 2. Batch Embedding (Optimization)

For bulk writes, batch embeddings to reduce provider API calls:

```rust
// In mom-embeddings crate
pub async fn batch_embed(
    embedder: &dyn Embedder,
    items: &mut [MemoryItem],
) -> anyhow::Result<()> {
    // Only embed items without embeddings
    let to_embed: Vec<(usize, String)> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.embedding.is_none())
        .filter_map(|(idx, item)| {
            item.content_text().map(|text| (idx, text.to_string()))
        })
        .collect();

    if to_embed.is_empty() {
        return Ok(());
    }

    // Embed in batches (e.g., 20 at a time)
    for batch in to_embed.chunks(20) {
        let texts: Vec<&str> = batch.iter().map(|(_, t)| t.as_str()).collect();
        let embeddings = embedder.batch_embed(&texts).await?;

        for ((idx, _), embedding) in batch.iter().zip(embeddings) {
            items[*idx].embedding = Some(embedding);
            items[*idx].embedding_model = Some(embedder.model_id().to_string());
        }
    }

    Ok(())
}
```

## Query Interface Changes

### 1. Add Vector Recall to MemoryStore Trait

**File**: `crates/mom-core/src/lib.rs`

```rust
#[async_trait::async_trait]
pub trait MemoryStore: Send + Sync {
    // ... existing methods ...

    /// Vector-based semantic search
    async fn vector_recall(
        &self,
        query_embedding: &[f32],
        scope: &ScopeKey,
        limit: usize,
    ) -> anyhow::Result<Vec<Scored<MemoryItem>>>;

    /// Hybrid recall: lexical + semantic with RRF fusion
    async fn hybrid_recall(
        &self,
        query: &Query,
        query_embedding: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<Scored<MemoryItem>>>;
}
```

### 2. SurrealDB Vector Query (SurrealQL)

```sql
-- Simple vector search (cosine similarity)
SELECT
    *,
    (vector::similarity::cosine(embedding, $query_vec)) AS similarity
FROM memory_items
WHERE tenant_id = $tenant_id
  AND vector::similarity::cosine(embedding, $query_vec) > 0.0
ORDER BY similarity DESC
LIMIT $limit;
```

### 3. Hybrid Query with RRF Fusion

**RRF Formula**:
```
score = 1 / (k + rank_lexical) + 1 / (k + rank_semantic)
```
where k=60 (typical constant)

```sql
-- Phase 1: Lexical results
LET $lexical = (
    SELECT id, ft::score() AS lex_score
    FROM memory_items
    WHERE tenant_id = $tenant_id
      AND content_text @@ $query_text  -- full-text match
    ORDER BY lex_score DESC
    LIMIT $limit
);

-- Phase 2: Vector results
LET $semantic = (
    SELECT id,
           vector::similarity::cosine(embedding, $query_vec) AS sem_score
    FROM memory_items
    WHERE tenant_id = $tenant_id
      AND embedding IS NOT NULL
    ORDER BY sem_score DESC
    LIMIT $limit
);

-- Phase 3: Merge with RRF scoring
SELECT
    id,
    (1.0 / (60 + $lex_rank) + 1.0 / (60 + $sem_rank)) AS rrf_score
FROM (
    SELECT id FROM $lexical  -- rank 1..limit
    UNION
    SELECT id FROM $semantic  -- rank 1..limit
)
ORDER BY rrf_score DESC
LIMIT $limit;
```

## Configuration

### Environment Variables

```bash
# Embedding Provider Selection
EMBEDDING_PROVIDER=ollama  # or: mistral, openai

# Ollama
OLLAMA_BASE_URL=http://localhost:11434
OLLAMA_MODEL=mxbai-embed-large

# Mistral
MISTRAL_API_KEY=...
MISTRAL_MODEL=mistral-embed

# OpenAI
OPENAI_API_KEY=...
OPENAI_MODEL=text-embedding-3-large

# Hybrid Search Weights (Phase 2)
HYBRID_LEXICAL_WEIGHT=0.5
HYBRID_SEMANTIC_WEIGHT=0.5
```

## Testing Strategy

### Unit Tests

- **Embedder mocks**: Test with fixed embeddings
- **SurrealDB vector schema**: Verify index creation
- **RRF scoring**: Test fusion algorithm
- **Batch embedding**: Verify correct batching

### Integration Tests

1. **Vector Storage**: Write items with embeddings, verify retrieval
2. **Vector Recall**: Query with embeddings, verify ranking
3. **Hybrid Ranking**: Compare RRF fusion vs individual methods
4. **Performance**: Measure latency for 10K, 100K items
5. **Scope Isolation**: Verify tenant/scope filters + vector search

### Benchmarks

```bash
# Embedding latency
benchmark_embedding_500_items()

# Vector retrieval
benchmark_vector_recall_100k_items()

# Hybrid retrieval
benchmark_hybrid_recall_100k_items()
```

## Acceptance Criteria

- [x] Embedder trait already exists (Phase 1)
- [ ] MemoryItem has embedding field
- [ ] StoredItem updated for persistence
- [ ] SurrealDB schema with vector index
- [ ] Ollama embedder implementation
- [ ] Mistral embedder implementation
- [ ] Batch embedding helper
- [ ] MemoryStore has vector_recall() method
- [ ] SurrealQL vector query implemented
- [ ] Vector queries < 500ms for 100K items
- [ ] Tests: embedding storage, retrieval, accuracy
- [ ] Hybrid recall (Issue #12) implemented

## Timeline

- **Embedding Providers**: 2-3 days (Issue #10)
- **Hybrid Search**: 2-3 days (Issue #12)
- **Testing & Benchmarking**: 2-3 days
- **Documentation & Examples**: 1-2 days

---

## Rollout Plan

### Phase 2a: Vector Foundations (Issue #10) ✅
- Add embedding field to data model
- Implement embedding providers (Ollama, Mistral, OpenAI)
- Store embeddings in SurrealDB

### Phase 2b: Hybrid Search (Issue #12)
- Implement vector_recall
- Implement hybrid_recall with RRF fusion
- Benchmark and tune

### Phase 2c: Multi-Source Ingestion (Issue #29)
- Define ingestion protocol for external sources
- Implement oxidizedRAG connector (code memories)
- Implement oxidizedgraph connector (workflow/decision memories)
- Implement data-fabric connector (fact/knowledge memories)
- Route ingested data to appropriate memory types

### Phase 2d: Optimization (Future)
- Vector index tuning
- Batch embedding for background jobs
- Caching layer for repeated queries
- Multi-provider fallback

---

## References

- SurrealDB Vector Search: https://surrealdb.com/docs/surrealdb/querying/functions/vector
- Ollama Embeddings: https://github.com/ollama/ollama/blob/main/docs/api.md#generate-embeddings
- Mistral Embeddings: https://docs.mistral.ai/capabilities/embeddings/
- OpenAI Embeddings: https://platform.openai.com/docs/guides/embeddings
- RRF (Reciprocal Rank Fusion): https://en.wikipedia.org/wiki/Reciprocal_rank_fusion
