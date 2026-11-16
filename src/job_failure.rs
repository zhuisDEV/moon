//! Bounded failure diagnostics shared by source extraction and topic synthesis.
//! Provider messages and credentials must never enter these persistent fields.

pub const CODES: [&str; 9] = [
    "model_failed",
    "invalid_response",
    "interrupted",
    "runtime_unavailable",
    "runtime_setup_failed",
    "authentication_failed",
    "model_unavailable",
    "model_route_mismatch",
    "model_timeout",
];

pub(crate) fn retryable(code: &str) -> bool {
    matches!(
        code,
        "model_failed" | "invalid_response" | "interrupted" | "model_timeout"
    )
}

// Keep the historical category and its existing CHECK constraint intact. Schema
// 13 adds a separate, bounded diagnostic; no old job/history row is rewritten.
pub(crate) fn category(code: &str) -> &str {
    match code {
        "invalid_response" | "interrupted" => code,
        _ => "model_failed",
    }
}
