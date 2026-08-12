CREATE TABLE context_metrics (
    request_id TEXT PRIMARY KEY
        CHECK (length(request_id) = 32 AND request_id NOT GLOB '*[^0-9a-f]*'),
    occurred_at_ms INTEGER NOT NULL,
    retrieval_mode TEXT NOT NULL
        CHECK (retrieval_mode IN ('lexical', 'semantic', 'hybrid')),
    status TEXT NOT NULL CHECK (status IN ('ok', 'error')),
    duration_us INTEGER NOT NULL CHECK (duration_us >= 0),
    memory_count INTEGER NOT NULL DEFAULT 0 CHECK (memory_count >= 0),
    reference_count INTEGER NOT NULL DEFAULT 0 CHECK (reference_count >= 0),
    packet_chars INTEGER NOT NULL DEFAULT 0 CHECK (packet_chars >= 0),
    packet_truncated INTEGER NOT NULL DEFAULT 0
        CHECK (packet_truncated IN (0, 1)),
    adapter_injected INTEGER CHECK (adapter_injected IN (0, 1)),
    review_outcome TEXT CHECK (
        review_outcome IS NULL OR review_outcome IN (
            'useful',
            'partial',
            'false_negative',
            'false_positive',
            'correct_empty',
            'stale',
            'redundant'
        )
    ),
    expected_rank INTEGER CHECK (expected_rank IS NULL OR expected_rank >= 1),
    reviewed_at_ms INTEGER,
    CHECK (
        (review_outcome IS NULL AND reviewed_at_ms IS NULL) OR
        (review_outcome IS NOT NULL AND reviewed_at_ms IS NOT NULL)
    )
);

CREATE INDEX context_metrics_occurred
    ON context_metrics(occurred_at_ms DESC, request_id);

CREATE INDEX context_metrics_review
    ON context_metrics(review_outcome, occurred_at_ms DESC);

CREATE TABLE runtime_metrics (
    event_id TEXT PRIMARY KEY
        CHECK (length(event_id) = 32 AND event_id NOT GLOB '*[^0-9a-f]*'),
    occurred_at_ms INTEGER NOT NULL,
    event_kind TEXT NOT NULL
        CHECK (event_kind IN ('learning', 'embedding', 'compaction')),
    status TEXT NOT NULL CHECK (status IN ('ok', 'error', 'skipped')),
    duration_us INTEGER NOT NULL CHECK (duration_us >= 0),
    evidence_changed INTEGER CHECK (evidence_changed IN (0, 1)),
    learning_eligible INTEGER CHECK (learning_eligible IN (0, 1)),
    proposed_memories INTEGER CHECK (proposed_memories IS NULL OR proposed_memories >= 0),
    accepted_memories INTEGER CHECK (accepted_memories IS NULL OR accepted_memories >= 0),
    embedding_selected INTEGER CHECK (embedding_selected IS NULL OR embedding_selected >= 0),
    embedding_completed INTEGER CHECK (embedding_completed IS NULL OR embedding_completed >= 0),
    embedding_remaining INTEGER CHECK (embedding_remaining IS NULL OR embedding_remaining >= 0),
    compacted INTEGER CHECK (compacted IN (0, 1)),
    tokens_before INTEGER CHECK (tokens_before IS NULL OR tokens_before >= 0),
    tokens_after INTEGER CHECK (tokens_after IS NULL OR tokens_after >= 0)
);

CREATE INDEX runtime_metrics_occurred
    ON runtime_metrics(occurred_at_ms DESC, event_id);

CREATE INDEX runtime_metrics_kind
    ON runtime_metrics(event_kind, occurred_at_ms DESC);
