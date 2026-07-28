pub mod auth;
pub mod chunking;
pub mod embedding;
pub mod legacy;
pub mod memory;
pub mod model;
pub mod redaction;
pub mod server;
pub mod store;

pub use auth::{AuthCheck, AuthLevel, AuthResolver, AuthStatusReport, ModelOutcome};
pub use embedding::{EmbeddingProvider, HashEmbedding, LocalEmbedding};
pub use model::{
    ContextCitation, ContextMemory, ContextPacket, ContextReference, ContextRequest, DistillAction,
    DistillInput, DistillOutcome, EmbedReport, EvidenceInput, EvidenceOutcome, HealthReport,
    ImportReport, IngestDocument, IngestOutcome, LegacySearchHit, MemoryInput, SearchHit,
    SearchMode, SearchRequest, ShadowReport,
};
pub use store::Store;
