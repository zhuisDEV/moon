use std::io::{BufRead, Write};
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{ContextRequest, EmbeddingProvider, RuntimeMetricInput, SearchMode, Store};

const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    #[serde(flatten)]
    operation: Operation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Operation {
    Ping,
    Context {
        query: String,
        mode: String,
        limit: usize,
        scope: Option<String>,
        max_chars: usize,
        evidence_per_memory: usize,
        #[serde(default)]
        structured: bool,
        #[serde(default)]
        observe: bool,
    },
    ContextInjection {
        request_id: String,
        injected: bool,
    },
    RuntimeMetric {
        event_kind: String,
        status: String,
        duration_us: u64,
        #[serde(default)]
        evidence_changed: Option<bool>,
        #[serde(default)]
        learning_eligible: Option<bool>,
        #[serde(default)]
        proposed_memories: Option<usize>,
        #[serde(default)]
        accepted_memories: Option<usize>,
        #[serde(default)]
        compacted: Option<bool>,
        #[serde(default)]
        tokens_before: Option<usize>,
        #[serde(default)]
        tokens_after: Option<usize>,
    },
    Embed {
        limit: usize,
    },
}

#[derive(Debug, Serialize)]
struct Response {
    id: Option<u64>,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Serve bounded JSON-lines requests over private stdin/stdout. The caller owns
/// this child process, so the model stays warm without a socket or standalone
/// daemon.
pub fn serve_stdio(
    store: &mut Store,
    provider: &dyn EmbeddingProvider,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<()> {
    for line in input.lines() {
        let line = line.context("read stdio request")?;
        if line.len() > MAX_REQUEST_BYTES {
            write_response(
                &mut output,
                Response {
                    id: None,
                    ok: false,
                    result: None,
                    error: Some(format!(
                        "request exceeds the maximum size of {MAX_REQUEST_BYTES} bytes"
                    )),
                },
            )?;
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                write_response(
                    &mut output,
                    Response {
                        id: None,
                        ok: false,
                        result: None,
                        error: Some(format!("invalid JSON request: {error}")),
                    },
                )?;
                continue;
            }
        };
        let id = request.id;
        let result = handle_request(store, provider, request.operation);
        let response = match result {
            Ok(result) => Response {
                id: Some(id),
                ok: true,
                result: Some(result),
                error: None,
            },
            Err(error) => Response {
                id: Some(id),
                ok: false,
                result: None,
                error: Some(crate::redaction::redact_text(&error.to_string()).value),
            },
        };
        write_response(&mut output, response)?;
    }
    Ok(())
}

fn handle_request(
    store: &mut Store,
    provider: &dyn EmbeddingProvider,
    operation: Operation,
) -> Result<serde_json::Value> {
    match operation {
        Operation::Ping => Ok(serde_json::json!({
            "provider": provider.name(),
            "model": provider.model(),
        })),
        Operation::Context {
            query,
            mode,
            limit,
            scope,
            max_chars,
            evidence_per_memory,
            structured,
            observe,
        } => {
            let mode = SearchMode::from_str(&mode).map_err(anyhow::Error::msg)?;
            let request = ContextRequest {
                query,
                mode,
                limit,
                scope,
                max_chars,
                evidence_per_memory,
            };
            let context_provider = (mode != SearchMode::Lexical).then_some(provider);
            if observe {
                let observation = store.observe_context(&request, context_provider)?;
                let packet = observation.packet;
                if structured {
                    return Ok(serde_json::json!({
                        "request_id": observation.request_id,
                        "packet": packet,
                    }));
                }
                let rendered = (!packet.is_empty()).then(|| packet.render_markdown());
                return Ok(serde_json::json!({
                    "request_id": observation.request_id,
                    "packet": rendered,
                    "memory_count": packet.memories.len(),
                    "reference_count": packet.references.len(),
                    "packet_chars": packet.used_chars,
                    "truncated": packet.truncated,
                }));
            }
            let packet = store.assemble_context(&request, context_provider)?;
            if structured {
                Ok(serde_json::to_value(packet)?)
            } else if packet.is_empty() {
                Ok(serde_json::Value::Null)
            } else {
                Ok(serde_json::Value::String(packet.render_markdown()))
            }
        }
        Operation::ContextInjection {
            request_id,
            injected,
        } => {
            store.mark_context_injected(&request_id, injected)?;
            Ok(serde_json::json!({"updated": true}))
        }
        Operation::RuntimeMetric {
            event_kind,
            status,
            duration_us,
            evidence_changed,
            learning_eligible,
            proposed_memories,
            accepted_memories,
            compacted,
            tokens_before,
            tokens_after,
        } => {
            let event_id = store.record_runtime_metric(&RuntimeMetricInput {
                event_kind,
                status,
                duration_us,
                evidence_changed,
                learning_eligible,
                proposed_memories,
                accepted_memories,
                compacted,
                tokens_before,
                tokens_after,
                ..RuntimeMetricInput::default()
            })?;
            Ok(serde_json::json!({"event_id": event_id}))
        }
        Operation::Embed { limit } => Ok(serde_json::to_value(
            store.observe_embeddings(provider, limit.clamp(1, 1_000))?,
        )?),
    }
}

fn write_response(output: &mut impl Write, response: Response) -> Result<()> {
    serde_json::to_writer(&mut *output, &response)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::serve_stdio;
    use crate::{HashEmbedding, MemoryInput, Store};

    #[test]
    fn stdio_server_keeps_provider_and_store_alive_across_requests() {
        let temp = tempfile::tempdir().expect("tempdir");
        let mut store = Store::open(temp.path().join("moon.sqlite"), 64).expect("store");
        store
            .remember(MemoryInput {
                memory_kind: "preference".to_string(),
                scope: "global".to_string(),
                title: Some("Style".to_string()),
                content: "The user prefers concise answers.".to_string(),
                importance: 0.8,
                confidence: 1.0,
                pinned: false,
            })
            .expect("remember");
        let provider = HashEmbedding::new(64);
        let requests = concat!(
            "{\"id\":1,\"op\":\"embed\",\"limit\":10}\n",
            "{\"id\":2,\"op\":\"context\",\"query\":\"concise answers\",",
            "\"mode\":\"hybrid\",\"limit\":4,\"scope\":null,\"max_chars\":2000,",
            "\"evidence_per_memory\":0,\"structured\":false}\n"
        );
        let mut output = Vec::new();
        serve_stdio(
            &mut store,
            &provider,
            std::io::Cursor::new(requests),
            &mut output,
        )
        .expect("serve");
        let responses = String::from_utf8(output).expect("utf8");
        let lines = responses.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"embedded\":1"));
        assert!(lines[1].contains("# Moon Context"));
    }
}
