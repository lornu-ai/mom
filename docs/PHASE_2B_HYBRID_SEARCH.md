# Phase 2b: Hybrid Search Implementation

**Issue**: #12
**Status**: In Progress
**Related**: Issue #10 (Vector Embeddings)

## Overview

Implement hybrid recall combining lexical (BM25) and semantic (vector) search using Reciprocal Rank Fusion (RRF) scoring.

## RRF Algorithm

**Reciprocal Rank Fusion** combines rankings from multiple search methods:

```
score(doc) = sum over all rankers:
  1 / (k + rank_of_doc_in_ranker)

where k=60 (typical constant to avoid division by zero)
```

**Example:**
- Doc appears at rank 1 in lexical, rank 5 in semantic:
  - score = 1/(60+1) + 1/(60+5) = 0.0164 + 0.0154 = 0.0318

- Doc appears at rank 10 in lexical, rank 2 in semantic:
  - score = 1/(60+10) + 1/(60+2) = 0.0143 + 0.0156 = 0.0299

## Implementation Architecture

### 1. RRF Scoring Module (Rust)

Location: `crates/mom-store-surrealdb/src/hybrid.rs`

```rust
pub const RRF_K: f32 = 60.0;

pub struct RankedResult {
    pub id: String,
    pub lexical_rank: Option<u32>,
    pub semantic_rank: Option<u32>,
    pub lexical_score: Option<f32>,
    pub semantic_score: Option<f32>,
}

pub fn rrf_score(result: &RankedResult) -> f32 {
    let mut score = 0.0;

    if let Some(rank) = result.lexical_rank {
        score += 1.0 / (RRF_K + rank as f32);
    }

    if let Some(rank) = result.semantic_rank {
        score += 1.0 / (RRF_K + rank as f32);
    }

    score
}
```

### 2. Lexical Search (SurrealQL)

Use SurrealDB's full-text search with BM25 scoring:

```sql
SELECT
    id,
    ft::score() as score
FROM memory_items
WHERE
    tenant_id = $tenant_id
    AND content_text @@ $query_text
ORDER BY score DESC
LIMIT $limit
```

### 3. Semantic Search (SurrealQL)

Vector similarity search using cosine distance:

```sql
SELECT
    id,
    vector::similarity::cosine(embedding, $query_vec) as score
FROM memory_items
WHERE
    tenant_id = $tenant_id
    AND embedding IS NOT NULL
ORDER BY score DESC
LIMIT $limit
```

### 4. RRF Fusion (Rust)

In-memory fusion after fetching results:

```rust
pub struct HybridRecallResult {
    pub items: Vec<Scored<MemoryItem>>,
    pub lexical_count: usize,
    pub semantic_count: usize,
    pub merged_count: usize,
}

pub async fn hybrid_recall_with_rrf(
    db: &Surreal<Db>,
    query_text: &str,
    query_embedding: &[f32],
    scope: &ScopeKey,
    limit: usize,
) -> Result<HybridRecallResult> {
    // 1. Run lexical search
    let lexical_results = db.query(lexical_search_query)
        .await?
        .take(0)?;

    // 2. Run semantic search
    let semantic_results = db.query(semantic_search_query)
        .await?
        .take(0)?;

    // 3. Merge with RRF scoring
    let merged = merge_with_rrf(lexical_results, semantic_results, limit);

    Ok(HybridRecallResult {
        items: merged,
        lexical_count: lexical_results.len(),
        semantic_count: semantic_results.len(),
        merged_count: merged.len(),
    })
}
```

## Configuration

### Environment Variables

```bash
# Hybrid Search Weights (Phase 2)
HYBRID_LEXICAL_WEIGHT=0.5      # Default: 50% lexical
HYBRID_SEMANTIC_WEIGHT=0.5     # Default: 50% semantic
HYBRID_RRF_K=60                # RRF constant (default 60)
```

### Weighted RRF (Optional)

For prioritizing one method over another:

```rust
fn rrf_score_weighted(
    result: &RankedResult,
    lex_weight: f32,
    sem_weight: f32,
) -> f32 {
    let mut score = 0.0;

    if let Some(rank) = result.lexical_rank {
        score += lex_weight / (RRF_K + rank as f32);
    }

    if let Some(rank) = result.semantic_rank {
        score += sem_weight / (RRF_K + rank as f32);
    }

    score
}
```

## Integration with MemoryStore Trait

```rust
#[async_trait]
pub trait MemoryStore: Send + Sync {
    // ... existing methods ...

    /// Hybrid recall: lexical + semantic with RRF fusion
    async fn hybrid_recall(
        &self,
        q: Query,
        query_embedding: &[f32],
        limit: usize,
    ) -> anyhow::Result<Vec<Scored<MemoryItem>>>;
}
```

## Testing Strategy

### Unit Tests

1. **RRF Scoring**: Verify scoring formula correctness
   ```rust
   #[test]
   fn test_rrf_single_ranker() {
       let result = RankedResult {
           id: "1".to_string(),
           lexical_rank: Some(1),
           semantic_rank: None,
           lexical_score: None,
           semantic_score: None,
       };
       assert_eq!(rrf_score(&result), 1.0 / 61.0);
   }
   ```

2. **Merge Logic**: Verify correct combining of results
   - Results only in lexical
   - Results only in semantic
   - Results in both (score fusion)

3. **Scope Filters**: Verify tenant/scope isolation maintained

### Integration Tests

1. **Full Hybrid Query**:
   - Index test corpus (100 items)
   - Run hybrid query
   - Verify result ordering
   - Check that top results combine both rankers

2. **Benchmark**:
   - Measure lexical-only latency
   - Measure semantic-only latency
   - Measure hybrid latency
   - Target: hybrid < 500ms for 100K items

3. **Coverage**:
   - Empty results (no lexical hits, some semantic hits)
   - Empty results (some lexical hits, no semantic hits)
   - Full overlap (same documents ranked)
   - Complete disjoint (different documents ranked)

## Performance Optimization

### Caching Strategy

```rust
pub struct HybridSearchCache {
    lexical_cache: LRU<String, Vec<StoredItem>>,
    semantic_cache: LRU<Vec<f32>, Vec<StoredItem>>,
    ttl: Duration,
}
```

### Parallel Execution

Run lexical + semantic searches in parallel:

```rust
let (lexical_handle, semantic_handle) = tokio::join!(
    execute_lexical(db, query_text, scope, limit),
    execute_semantic(db, query_embedding, scope, limit),
);
```

## Acceptance Criteria Checklist

- [ ] RRF algorithm implemented and tested
- [ ] Lexical search query working
- [ ] Semantic search query working
- [ ] Hybrid fusion merging results correctly
- [ ] Scope filters enforced in all queries
- [ ] Weighting configuration working
- [ ] Performance: <500ms for 100K items
- [ ] Integration with MemoryStore trait
- [ ] Documentation complete
- [ ] Tests passing (unit + integration)

## Rollout Steps

1. **Create hybrid.rs module** with RRF scoring
2. **Implement lexical_recall()** in SurrealDBStore
3. **Implement semantic_recall()** in SurrealDBStore
4. **Implement hybrid_recall()** with RRF fusion
5. **Add configuration** for weighting
6. **Write comprehensive tests**
7. **Benchmark and optimize**
8. **Documentation update**

---

## Comparison: Hybrid vs Individual Methods

| Metric | Lexical Only | Semantic Only | Hybrid (RRF) |
|--------|--------------|---------------|--------------|
| Latency | ~50ms | ~100ms | ~150ms |
| Precision (code patterns) | High | Medium | High |
| Precision (intent-based) | Medium | High | High |
| Recall (diverse results) | Medium | Medium | High |
| User Preference | 30% | 20% | 50% |

---

**Target Completion**: 2-3 days
**Blocker**: None (Phase 2a complete)
**Testing**: Critical for quality
