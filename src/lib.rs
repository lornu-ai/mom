//! MOM - Memory for Autonomous Agents
//!
//! Event-sourced memory kernel + retrieval engine.

pub mod memory;
pub mod store;
pub mod error;

pub use memory::{MemoryItem, MemoryKind, ScopeKey, Source, Content};
pub use store::{MemoryStore, Query, ScoredMemory};
pub use error::{MomError, Result};

#[derive(Debug, Clone)]
pub struct ContextPack {
    pub highlights: Vec<MemoryItem>,
    pub summaries: Vec<MemoryItem>,
    pub facts: Vec<MemoryItem>,
    pub citations: Vec<Citation>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Citation {
    pub source_id: String,
    pub target_id: String,
    pub relation: String,
}
