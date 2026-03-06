# Phase 2c: Multi-Source Ingestion Architecture

**Issue**: #29
**Status**: In Progress
**Scope**: Integrate oxidizedRAG, oxidizedgraph, data-fabric as memory sources

## Vision

MOM becomes the **unified memory layer** for autonomous agent ecosystem:

```
┌──────────────────┐
│  oxidizedRAG     │  Code understanding
│  (code context)  │  + pattern analysis
└────────┬─────────┘
         │
         ├──→ Code Memories (Events + Facts)
         │
┌────────▼─────────┐
│      MOM         │  Central Memory Hub
│  (All agents)    │  Single source of truth
└────────┬─────────┘
         │
         ├←──┐
         │   │
         │   ├──→ Memory Queries (recall)
         │   └──→ Context Packs (agents)
         │
┌────────▼─────────┐
│ oxidizedgraph    │  Workflow orchestration
│ (decisions)      │  + execution traces
└──────────────────┘

┌────────────────────┐
│  data-fabric       │  Task/modification logs
│  (facts)           │  + durable knowledge
└────────────────────┘
```

## Ingestion Protocol

### Core Trait

```rust
#[async_trait]
pub trait MemorySource: Send + Sync {
    /// Unique identifier for this source
    fn source_id(&self) -> &str;

    /// Describe what this source provides
    fn description(&self) -> &str;

    /// Fetch memories from this source
    async fn fetch_memories(
        &self,
        scope: &ScopeKey,
        since: Option<i64>,  // Optional: only changes since timestamp
    ) -> anyhow::Result<Vec<MemoryItem>>;

    /// Optional: subscribe to real-time updates
    async fn subscribe_updates(
        &self,
        scope: &ScopeKey,
        callback: Box<dyn Fn(MemoryItem) + Send>,
    ) -> anyhow::Result<()>;
}
```

### Ingestion Flow

```
MemorySource.fetch_memories()
    ↓
[Transform to MemoryItem]
    ├─ Set source field = source_id
    ├─ Map scope (tenant/workspace/agent/run)
    ├─ Extract/generate embedding if needed
    └─ Assign memory type (Event/Fact/Summary)
    ↓
MemoryStore.put()
    ↓
[Stored in SurrealDB]
```

## Source Implementations

### 1. oxidizedRAG Connector

**File**: `crates/mom-sources/src/oxidizedrag.rs`

**What it provides:**
- Code analysis results (AST patterns, semantic meanings)
- Cross-file dependencies and relationships
- Function/class definitions and call chains

**Memory types:**
- `Event`: Raw code observations ("file X modified")
- `Fact`: Extracted patterns ("function Y calls Z")
- `Summary`: Code summaries and documentation

**Scope mapping:**
```rust
CodeContext {
    repo: "github.com/user/repo",
    file_path: "src/main.rs",
    language: "rust",
}
    ↓
ScopeKey {
    tenant_id: "user",
    workspace_id: Some("repo"),
    project_id: Some("main.rs"),
    agent_id: None,
    run_id: None,
}
```

**Example memory:**
```json
{
  "id": "oxidizedrag:func:analyze:main",
  "kind": "Fact",
  "content": {
    "text": "Function main() in src/main.rs calls initialize()",
    "json": {
      "function": "main",
      "calls": ["initialize"],
      "defined_in": "src/main.rs"
    }
  },
  "source": "oxidizedrag",
  "tags": ["code-analysis", "rust", "function-call"],
  "importance": 0.8,
  "confidence": 0.95,
  "meta": {
    "analysis_model": "tree-sitter-rust",
    "ast_depth": 3
  }
}
```

### 2. oxidizedgraph Connector

**File**: `crates/mom-sources/src/oxidizedgraph.rs`

**What it provides:**
- Agent workflow executions
- State transitions and decisions
- Task completions and failures
- Agent reasoning traces

**Memory types:**
- `Event`: Workflow events ("agent started task X")
- `Summary`: Episode summaries ("agent completed workflow Y")
- `Fact`: Extracted decisions ("agent chose strategy Z")

**Scope mapping:**
```rust
WorkflowExecution {
    agent_id: "agent:code-reviewer",
    run_id: "run:20260305:abc123",
    workspace_id: "workspace:ci-cd",
}
    ↓
ScopeKey {
    tenant_id: "lornu",
    workspace_id: Some("ci-cd"),
    project_id: Some("code-review"),
    agent_id: Some("code-reviewer"),
    run_id: Some("20260305:abc123"),
}
```

**Example memory:**
```json
{
  "id": "oxidizedgraph:run:20260305:abc123:decision:1",
  "kind": "Event",
  "content": {
    "text": "Agent code-reviewer decided to run linter on modified files",
    "json": {
      "agent": "code-reviewer",
      "decision": "run_linter",
      "context": ["modified_files", "lint_rules"],
      "confidence": 0.92
    }
  },
  "source": "oxidizedgraph",
  "tags": ["workflow", "decision", "linting"],
  "importance": 0.7,
  "confidence": 0.92,
  "meta": {
    "run_duration_ms": 5432,
    "task_order": 1
  }
}
```

### 3. data-fabric Connector

**File**: `crates/mom-sources/src/datafabric.rs`

**What it provides:**
- Task records and metadata
- File modifications and changes
- Durable knowledge base entries
- Fact validations and conflicts

**Memory types:**
- `Event`: Task execution ("task:build:success")
- `Fact`: Validated facts ("pattern:async-safety:verified")
- `Preference`: Policies ("retry:exponential-backoff")

**Scope mapping:**
```rust
TaskRecord {
    workspace_id: "github.com/user/repo",
    task_id: "build:ci-20260305",
    entity_type: "task",
}
    ↓
ScopeKey {
    tenant_id: "user",
    workspace_id: Some("repo"),
    project_id: Some("ci"),
    agent_id: None,
    run_id: Some("20260305"),
}
```

**Example memory:**
```json
{
  "id": "datafabric:task:build:20260305",
  "kind": "Fact",
  "content": {
    "text": "Build task completed successfully on 2026-03-05",
    "json": {
      "task": "build",
      "status": "success",
      "duration_ms": 120000,
      "artifacts": ["build/app.wasm", "dist/"]
    }
  },
  "source": "datafabric",
  "tags": ["build", "ci", "success"],
  "importance": 0.6,
  "confidence": 1.0,
  "meta": {
    "task_type": "ci_build",
    "commit_hash": "abc123def456"
  }
}
```

## Ingestion Scheduler

```rust
pub struct IngestionScheduler {
    sources: Vec<Box<dyn MemorySource>>,
    store: Arc<dyn MemoryStore>,
    poll_interval_secs: u64,
}

impl IngestionScheduler {
    pub async fn run(&self) {
        loop {
            for source in &self.sources {
                match source.fetch_memories(&scope, None).await {
                    Ok(items) => {
                        for item in items {
                            let _ = self.store.put(item).await;
                        }
                    }
                    Err(e) => {
                        warn!("Source {} fetch failed: {}", source.source_id(), e);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(self.poll_interval_secs)).await;
        }
    }
}
```

## Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum IngestionError {
    #[error("Source {0} unavailable: {1}")]
    SourceUnavailable(String, String),

    #[error("Invalid memory format: {0}")]
    InvalidMemory(String),

    #[error("Scope mismatch: {0}")]
    ScopeMismatch(String),

    #[error("Storage error: {0}")]
    StorageError(#[from] anyhow::Error),
}
```

## Testing Strategy

### Unit Tests
1. **Scope mapping**: Verify correct ScopeKey generation
2. **Memory type assignment**: Ensure Events/Facts routed correctly
3. **Source provenance**: Track source_id in metadata
4. **Error handling**: Handle missing fields gracefully

### Integration Tests
1. **Mock sources**: Simulate each source with test data
2. **End-to-end flow**: Fetch → Transform → Store → Query
3. **Multi-source consistency**: Overlapping memories handled correctly
4. **Scope isolation**: Verify tenant/workspace boundaries

### Performance Tests
1. **Batch ingestion**: 1000 items/sec throughput
2. **Concurrent sources**: 3 sources running in parallel
3. **Storage latency**: <10ms per write

## Acceptance Criteria

- [ ] MemorySource trait defined and documented
- [ ] oxidizedRAG connector implemented
- [ ] oxidizedgraph connector implemented
- [ ] data-fabric connector implemented
- [ ] Scope mapping correct for all sources
- [ ] Source provenance tracked (source_id in metadata)
- [ ] IngestionScheduler polls and stores memories
- [ ] Error handling for source failures
- [ ] Unit tests passing (95%+ coverage)
- [ ] Integration tests with mock sources
- [ ] Performance benchmarks documented
- [ ] Real-world test with live sources (Phase 2d)

## Rollout Steps

### Phase 2c.1: Framework
1. Create mom-sources crate
2. Define MemorySource trait
3. Implement IngestionScheduler
4. Write test utilities and mocks

### Phase 2c.2: Connectors
1. oxidizedRAG connector (fetch code analysis)
2. oxidizedgraph connector (fetch workflow traces)
3. data-fabric connector (fetch task records)

### Phase 2c.3: Testing & Integration
1. Unit tests for all connectors
2. Integration tests with mocks
3. Real-world testing with live systems
4. Performance benchmarks

### Phase 2c.4: Observability
1. Ingestion metrics (items/sec, errors)
2. Source health monitoring
3. Memory reconciliation (detect conflicts)

## Success Metrics

| Metric | Target | Status |
|--------|--------|--------|
| Fetch latency | <100ms per source | TBD |
| Throughput | >1000 items/sec | TBD |
| Uptime | 99.9% availability | TBD |
| Error rate | <0.1% | TBD |
| Coverage | All 3 sources | TBD |

---

**Milestone**: By end of Phase 2c, MOM is the unified memory hub for entire lornu-ai ecosystem.

