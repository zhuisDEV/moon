use anyhow::Result;
use serde_json::Value;

use crate::commands::CommandReport;
use crate::moon::paths::resolve_paths;
use crate::moon::qmd;
use crate::moon::util::truncate_with_ellipsis;

#[derive(Debug, Clone)]
pub struct MoonRecallOptions {
    pub collection_name: String,
    pub query: String,
    pub limit: usize,
}

#[derive(Debug, Clone, Default)]
struct RecallHit {
    score: Option<f64>,
    source: Option<String>,
    text: Option<String>,
}

pub fn run(opts: &MoonRecallOptions) -> Result<CommandReport> {
    let paths = resolve_paths()?;
    qmd::install_runtime_env(&paths);
    let mut report = CommandReport::new("recall");

    let query = opts.query.trim();
    if query.is_empty() {
        report.issue("recall query cannot be empty".to_string());
        return Ok(report);
    }

    let limit = opts.limit.max(1);
    let exec = qmd::recall_query(
        &paths.qmd_bin,
        &opts.collection_name,
        query,
        limit,
        Some(60),
    );
    let exec = match exec {
        Ok(exec) => exec,
        Err(err) => {
            report.issue(format!("{err:#}"));
            return Ok(report);
        }
    };

    report.detail(format!("recall.collection={}", opts.collection_name));
    report.detail(format!("recall.mode={}", exec.mode));
    report.detail(format!("recall.limit={limit}"));

    match serde_json::from_str::<Value>(&exec.stdout) {
        Ok(value) => {
            let hits = extract_hits(&value);
            report.detail(format!("recall.result_count={}", hits.len()));
            if hits.is_empty() {
                report.detail("recall.no_results=true".to_string());
            }
            for (idx, hit) in hits.iter().enumerate() {
                if let Some(score) = hit.score {
                    report.detail(format!("hit[{}].score={score:.4}", idx + 1));
                }
                if let Some(source) = hit.source.as_deref() {
                    report.detail(format!("hit[{}].source={source}", idx + 1));
                }
                if let Some(text) = hit.text.as_deref() {
                    report.detail(format!("hit[{}].text={text}", idx + 1));
                }
            }
        }
        Err(_) => {
            let trimmed = truncate_with_ellipsis(exec.stdout.trim(), 800);
            if trimmed.is_empty() {
                report.detail("recall.result_count=0".to_string());
                report.detail("recall.no_results=true".to_string());
            } else {
                report.detail("recall.result_count=unknown".to_string());
                report.detail(format!("recall.raw_output={trimmed}"));
            }
        }
    }

    if !exec.stderr.trim().is_empty() {
        report.detail(format!(
            "recall.stderr={}",
            truncate_with_ellipsis(exec.stderr.trim(), 300)
        ));
    }

    Ok(report)
}

fn extract_hits(value: &Value) -> Vec<RecallHit> {
    let Some(items) = result_array(value) else {
        return Vec::new();
    };

    items.iter().filter_map(extract_hit).collect()
}

fn result_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(items) = value.as_array() {
        return Some(items);
    }

    for key in ["results", "matches", "items", "hits", "data"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            return Some(items);
        }
    }

    None
}

fn extract_hit(value: &Value) -> Option<RecallHit> {
    let source = first_string(
        value,
        &[
            &["path"],
            &["source"],
            &["id"],
            &["docId"],
            &["document", "path"],
            &["document", "id"],
            &["metadata", "path"],
            &["metadata", "source"],
        ],
    );
    let text = first_string(
        value,
        &[
            &["snippet"],
            &["text"],
            &["content"],
            &["body"],
            &["summary"],
            &["document", "text"],
            &["document", "content"],
            &["metadata", "snippet"],
        ],
    )
    .map(|text| truncate_with_ellipsis(&text, 240));
    let score = first_f64(value, &[&["score"], &["_score"], &["relevance"]]);

    if source.is_none() && text.is_none() && score.is_none() {
        return None;
    }

    Some(RecallHit {
        score,
        source,
        text,
    })
}

fn first_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    for path in paths {
        let mut cursor = value;
        let mut found = true;
        for part in *path {
            let Some(next) = cursor.get(*part) else {
                found = false;
                break;
            };
            cursor = next;
        }
        if found && let Some(text) = cursor.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn first_f64(value: &Value, paths: &[&[&str]]) -> Option<f64> {
    for path in paths {
        let mut cursor = value;
        let mut found = true;
        for part in *path {
            let Some(next) = cursor.get(*part) else {
                found = false;
                break;
            };
            cursor = next;
        }
        if found {
            if let Some(number) = cursor.as_f64() {
                return Some(number);
            }
            if let Some(number) = cursor.as_i64() {
                return Some(number as f64);
            }
            if let Some(number) = cursor.as_u64() {
                return Some(number as f64);
            }
        }
    }
    None
}
