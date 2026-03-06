# MOM - Memory for All Autonomous Agents

Dedicated to all Mothers out there. 🌹

**MOM** is an **event-sourced memory kernel + retrieval engine** for autonomous agents of all types.

## Quick Start

```bash
cargo build && cargo run --bin mom
```

## Implementation Plan

- **Axum HTTP API** (REST endpoints)
- **SQLite** (default backend, optional Postgres)
- **Hierarchical memory** (short → mid → long consolidation)
- **Hybrid retrieval** (vector + BM25 + graph edges)
- **Multi-tenant** with ACL support
- **Optional TypeScript/Bun SDK**

See [docs/DESIGN.md](./docs/DESIGN.md) for full specification.
