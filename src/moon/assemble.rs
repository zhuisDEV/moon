use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::moon::distill::load_source_excerpt;
use crate::moon::files::{
    ensure_readable_file, file_epoch_secs, latest_file_with_extension, resolve_session_id,
};
use crate::moon::paths::MoonPaths;
use crate::moon::state::{
    LIBRARY_EMBED_COLLECTION, MoonState, count_embedded_projection_docs_under,
    embedded_projection_epoch, hot_embed_collection_for_session, hot_projection_path_for_session,
    is_hot_embed_collection,
};
use crate::moon::util::now_epoch_secs;

#[derive(Debug, Clone)]
pub struct AssembleInput {
    pub session_id: String,
    pub raw_source_path: PathBuf,
    pub cleanse_summary_path: Option<PathBuf>,
    pub embedding_index_anchor: Option<EmbeddingIndexAnchor>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingIndexAnchor {
    pub hot_collection: String,
    pub hot_indexed_projection_docs: usize,
    pub library_collection: String,
    pub library_indexed_projection_docs: usize,
    pub pending_embed_collections: Vec<String>,
    pub last_embed_trigger_epoch_secs: Option<u64>,
    pub hot_session_projection_path: Option<String>,
    pub hot_session_projection_indexed: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct AssembleOutput {
    pub session_id: String,
    pub raw_source_path: String,
    pub cleanse_summary_path: Option<String>,
    pub content: String,
    pub assembled_at_epoch_secs: u64,
}

pub fn output_path(paths: &MoonPaths, session_id: &str) -> PathBuf {
    paths.context_engine_dir.join(format!("{session_id}.md"))
}

pub fn resolve_input(
    paths: &MoonPaths,
    source_path: Option<&str>,
    session_id: Option<&str>,
) -> Result<AssembleInput> {
    let raw_source_path = select_raw_source_path(paths, source_path, session_id)?;
    let session_id = resolve_session_id(session_id, &raw_source_path)?;
    let cleanse_summary_path = matching_cleanse_summary_path(paths, &session_id);

    Ok(AssembleInput {
        session_id,
        raw_source_path,
        cleanse_summary_path,
        embedding_index_anchor: None,
    })
}

pub fn embedding_index_anchor_from_state(
    paths: &MoonPaths,
    state: &MoonState,
    session_id: &str,
) -> EmbeddingIndexAnchor {
    let pending_embed_collections = state
        .pending_embed_collections
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let hot_collection = hot_embed_collection_for_session(session_id);
    let session_projection_path_buf = hot_projection_path_for_session(paths, session_id);
    let session_projection_path = session_projection_path_buf.display().to_string();
    let session_projection_indexed = if session_projection_path_buf.is_file() {
        let key = session_projection_path_buf.display().to_string();
        let projection_mtime = file_epoch_secs(&session_projection_path_buf);
        Some(
            embedded_projection_epoch(state, &hot_collection, &key)
                .is_some_and(|embedded_at| embedded_at >= projection_mtime),
        )
    } else {
        None
    };

    let hot_indexed_projection_docs =
        count_embedded_projection_docs_under(state, &hot_collection, &paths.mds_dir);
    let library_indexed_projection_docs =
        count_embedded_projection_docs_under(state, LIBRARY_EMBED_COLLECTION, &paths.mlib_dir);

    EmbeddingIndexAnchor {
        hot_collection,
        hot_indexed_projection_docs,
        library_collection: LIBRARY_EMBED_COLLECTION.to_string(),
        library_indexed_projection_docs,
        pending_embed_collections,
        last_embed_trigger_epoch_secs: state.last_embed_trigger_epoch_secs,
        hot_session_projection_path: Some(session_projection_path),
        hot_session_projection_indexed: session_projection_indexed,
    }
}

pub fn assemble_context(input: &AssembleInput) -> Result<AssembleOutput> {
    let raw_source_path = input
        .raw_source_path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("assemble raw source path is not valid UTF-8"))?;
    let raw_excerpt = load_source_excerpt(raw_source_path).with_context(|| {
        format!(
            "failed to derive assemble input from {}",
            input.raw_source_path.display()
        )
    })?;
    let cleanse_body = match input.cleanse_summary_path.as_ref() {
        Some(path) => Some(read_cleanse_body(path)?),
        None => None,
    };
    let assembled_at_epoch_secs = now_epoch_secs()?;
    let content = render_context(
        &input.session_id,
        raw_source_path,
        input.cleanse_summary_path.as_ref(),
        cleanse_body.as_deref(),
        &raw_excerpt,
        input.embedding_index_anchor.as_ref(),
        assembled_at_epoch_secs,
    );

    Ok(AssembleOutput {
        session_id: input.session_id.clone(),
        raw_source_path: raw_source_path.to_string(),
        cleanse_summary_path: input
            .cleanse_summary_path
            .as_ref()
            .map(|path| path.display().to_string()),
        content,
        assembled_at_epoch_secs,
    })
}

pub fn write_assembly_output(
    paths: &MoonPaths,
    session_id: &str,
    content: &str,
) -> Result<PathBuf> {
    fs::create_dir_all(&paths.context_engine_dir)
        .with_context(|| format!("failed to create {}", paths.context_engine_dir.display()))?;
    let path = output_path(paths, session_id);
    fs::write(&path, content.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn select_raw_source_path(
    paths: &MoonPaths,
    source_path: Option<&str>,
    session_id: Option<&str>,
) -> Result<PathBuf> {
    if let Some(source_path) = source_path
        && !source_path.trim().is_empty()
    {
        let path = PathBuf::from(source_path.trim());
        ensure_readable_file(&path, "assemble")?;
        return Ok(path);
    }

    if let Some(session_id) = session_id
        && !session_id.trim().is_empty()
    {
        let path = paths.raw_dir.join(format!("{}.jsonl", session_id.trim()));
        ensure_readable_file(&path, "assemble")?;
        return Ok(path);
    }

    latest_raw_file(&paths.raw_dir)?
        .ok_or_else(|| anyhow::anyhow!("no raw sources found under {}", paths.raw_dir.display()))
}

fn latest_raw_file(raw_dir: &Path) -> Result<Option<PathBuf>> {
    latest_file_with_extension(raw_dir, "jsonl", false)
}

fn matching_cleanse_summary_path(paths: &MoonPaths, session_id: &str) -> Option<PathBuf> {
    let path = paths.cleanse_dir.join(format!("{session_id}.md"));
    path.is_file().then_some(path)
}

fn read_cleanse_body(path: &Path) -> Result<String> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let body = strip_frontmatter(&raw).trim();
    if body.is_empty() {
        return Ok("- none".to_string());
    }
    Ok(body.to_string())
}

fn strip_frontmatter(raw: &str) -> &str {
    let Some(rest) = raw.strip_prefix("---\n") else {
        return raw;
    };
    let Some(idx) = rest.find("\n---\n") else {
        return raw;
    };
    &rest[idx + 5..]
}

fn render_context(
    session_id: &str,
    raw_source_path: &str,
    cleanse_summary_path: Option<&PathBuf>,
    cleanse_body: Option<&str>,
    raw_excerpt: &str,
    embedding_index_anchor: Option<&EmbeddingIndexAnchor>,
    assembled_at_epoch_secs: u64,
) -> String {
    let cleanse_summary_path = cleanse_summary_path.map(|path| path.display().to_string());
    let cleanse_body = cleanse_body.unwrap_or("- none");
    let raw_excerpt = if raw_excerpt.trim().is_empty() {
        "- none"
    } else {
        raw_excerpt.trim()
    };
    let embed_status = embedding_index_anchor
        .map(embed_anchor_status)
        .unwrap_or("unavailable");
    let hot_collection = embedding_index_anchor
        .map(|anchor| anchor.hot_collection.as_str())
        .unwrap_or("none");
    let hot_indexed_docs = embedding_index_anchor
        .map(|anchor| anchor.hot_indexed_projection_docs.to_string())
        .unwrap_or_else(|| "0".to_string());
    let library_collection = embedding_index_anchor
        .map(|anchor| anchor.library_collection.as_str())
        .unwrap_or(LIBRARY_EMBED_COLLECTION);
    let library_indexed_docs = embedding_index_anchor
        .map(|anchor| anchor.library_indexed_projection_docs.to_string())
        .unwrap_or_else(|| "0".to_string());
    let embed_pending = embedding_index_anchor
        .map(|anchor| {
            if anchor.pending_embed_collections.is_empty() {
                "none".to_string()
            } else {
                anchor.pending_embed_collections.join(",")
            }
        })
        .unwrap_or_else(|| "none".to_string());
    let embed_last_trigger = embedding_index_anchor
        .and_then(|anchor| anchor.last_embed_trigger_epoch_secs)
        .map(|epoch| epoch.to_string())
        .unwrap_or_else(|| "none".to_string());
    let embed_hot_projection = embedding_index_anchor
        .and_then(|anchor| anchor.hot_session_projection_path.as_deref())
        .unwrap_or("none");
    let embed_hot_indexed = embedding_index_anchor
        .and_then(|anchor| anchor.hot_session_projection_indexed)
        .map(|indexed| indexed.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let library_pending = embedding_index_anchor
        .map(|anchor| {
            anchor
                .pending_embed_collections
                .iter()
                .any(|collection| !is_hot_embed_collection(collection))
        })
        .map(|pending| pending.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    format!(
        "---\nmoon_assemble: 1\nsession_id: {}\nraw_source_path: {}\ncleanse_summary_path: {}\nassembled_at_epoch_secs: {}\n---\n\n# MOON Assembly Context\n\n## Control Summary\n- session_id: {}\n- cleanse_summary: {}\n- embedding_index_anchor: {}\n\n## Cleanse Summary\n{}\n\n## Embedding Index Anchor\n- status: {}\n- hot_collection: {}\n- hot_indexed_projection_docs: {}\n- hot_session_projection_path: {}\n- hot_session_projection_indexed: {}\n- library_collection: {}\n- library_indexed_projection_docs: {}\n- library_pending: {}\n- pending_embed_collections: {}\n- last_embed_trigger_epoch_secs: {}\n\n## Raw Context Excerpt\n{}\n",
        serde_json::to_string(session_id).unwrap_or_else(|_| "\"session\"".to_string()),
        serde_json::to_string(raw_source_path).unwrap_or_else(|_| "\"\"".to_string()),
        cleanse_summary_path
            .as_deref()
            .map(|path| serde_json::to_string(path).unwrap_or_else(|_| "\"\"".to_string()))
            .unwrap_or_else(|| "null".to_string()),
        assembled_at_epoch_secs,
        session_id,
        if cleanse_summary_path.is_some() {
            "present"
        } else {
            "none"
        },
        embed_status,
        cleanse_body,
        embed_status,
        hot_collection,
        hot_indexed_docs,
        embed_hot_projection,
        embed_hot_indexed,
        library_collection,
        library_indexed_docs,
        library_pending,
        embed_pending,
        embed_last_trigger,
        raw_excerpt
    )
}

fn embed_anchor_status(anchor: &EmbeddingIndexAnchor) -> &'static str {
    let hot_pending = anchor
        .pending_embed_collections
        .iter()
        .any(|collection| is_hot_embed_collection(collection));
    if anchor.hot_session_projection_indexed == Some(true) && !hot_pending {
        return "ready";
    }
    if anchor.hot_session_projection_indexed == Some(false) || hot_pending {
        return "pending";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::{assemble_context, embedding_index_anchor_from_state, resolve_input};
    use crate::moon::paths::MoonPaths;
    use crate::moon::state::{
        LIBRARY_EMBED_COLLECTION, MoonState, hot_embed_collection_for_session,
    };
    use serde_json::json;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> MoonPaths {
        MoonPaths {
            moon_home: root.to_path_buf(),
            raw_dir: root.join("raw"),
            mds_dir: root.join("mds"),
            mlib_dir: root.join("mlib"),
            cleanse_dir: root.join("cleanse"),
            memory_dir: root.join("memory"),
            memory_file: root.join("MEMORY.md"),
            logs_dir: root.join("logs"),
            context_engine_dir: root.join("mce"),
            openclaw_sessions_dir: root.join("sessions"),
            qmd_bin: root.join("bin/qmd"),
            qmd_db: root.join("qmd.sqlite"),
            qmd_config_dir: root.join("qmd-config"),
            moon_home_is_explicit: true,
        }
    }

    #[test]
    fn resolve_input_picks_matching_cleanse_summary() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.cleanse_dir).expect("mkdir cleanse");
        fs::write(
            paths.raw_dir.join("s1.jsonl"),
            "{\"message\":{\"role\":\"user\"}}\n",
        )
        .expect("write raw");
        fs::write(
            paths.cleanse_dir.join("s1.md"),
            "---\nmoon_cleanse: 1\n---\n\n# Cleanse Summary\n- keep only signal\n",
        )
        .expect("write cleanse");

        let input = resolve_input(&paths, None, Some("s1")).expect("resolve input");
        assert_eq!(input.session_id, "s1");
        assert_eq!(input.raw_source_path, paths.raw_dir.join("s1.jsonl"));
        assert_eq!(
            input.cleanse_summary_path,
            Some(paths.cleanse_dir.join("s1.md"))
        );
        assert!(input.embedding_index_anchor.is_none());
    }

    #[test]
    fn assemble_context_renders_cleanse_summary_and_raw_excerpt() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.cleanse_dir).expect("mkdir cleanse");

        let raw = paths.raw_dir.join("s1.jsonl");
        let user = json!({
            "message": {
                "role": "user",
                "content": [{"type":"text","text":"Capture the current plan."}]
            }
        });
        let assistant = json!({
            "message": {
                "role": "assistant",
                "content": [{"type":"text","text":"I will keep the primary path clean."}]
            }
        });
        fs::write(&raw, format!("{user}\n{assistant}\n")).expect("write raw");

        fs::write(
            paths.cleanse_dir.join("s1.md"),
            "---\nmoon_cleanse: 1\nsession_id: \"s1\"\n---\n\n# Cleanse Summary\n## Decisions\n- Keep `cleanse` separate from `project`.\n",
        )
        .expect("write cleanse");

        let input = resolve_input(&paths, None, Some("s1")).expect("resolve input");
        let output = assemble_context(&input).expect("assemble context");

        assert_eq!(output.session_id, "s1");
        assert_eq!(output.raw_source_path, raw.display().to_string());
        assert_eq!(
            output.cleanse_summary_path,
            Some(paths.cleanse_dir.join("s1.md").display().to_string())
        );
        assert!(output.assembled_at_epoch_secs > 0);
        assert!(output.content.contains("moon_assemble: 1"));
        assert!(output.content.contains("# MOON Assembly Context"));
        assert!(output.content.contains("# Cleanse Summary"));
        assert!(
            output
                .content
                .contains("Keep `cleanse` separate from `project`.")
        );
        assert!(output.content.contains("Capture the current plan."));
        assert!(
            output
                .content
                .contains("I will keep the primary path clean.")
        );
        assert!(output.content.contains("## Embedding Index Anchor"));
        assert!(output.content.contains("- status: unavailable"));
        assert!(!output.content.contains("moon_cleanse: 1"));
    }

    #[test]
    fn assemble_context_marks_missing_cleanse_summary_as_none() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");

        fs::write(
            paths.raw_dir.join("s2.jsonl"),
            "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Only raw context exists.\"}]}}\n",
        )
        .expect("write raw");

        let input = resolve_input(&paths, None, Some("s2")).expect("resolve input");
        let output = assemble_context(&input).expect("assemble context");

        assert!(output.cleanse_summary_path.is_none());
        assert!(output.content.contains("- cleanse_summary: none"));
        assert!(output.content.contains("## Cleanse Summary\n- none"));
        assert!(output.content.contains("## Embedding Index Anchor"));
        assert!(output.content.contains("- status: unavailable"));
        assert!(output.content.contains("Only raw context exists."));
    }

    #[test]
    fn assemble_context_renders_embedding_index_anchor_from_state() {
        let tmp = tempdir().expect("tempdir");
        let moon_home = tmp.path().join("moon-home");
        let paths = test_paths(&moon_home);
        fs::create_dir_all(&paths.raw_dir).expect("mkdir raw");
        fs::create_dir_all(&paths.mds_dir).expect("mkdir mds");
        fs::create_dir_all(&paths.mlib_dir).expect("mkdir mlib");

        fs::write(
            paths.raw_dir.join("s3.jsonl"),
            "{\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Use the index anchor.\"}]}}\n",
        )
        .expect("write raw");
        let projection = crate::moon::state::hot_projection_path_for_session(&paths, "s3");
        fs::create_dir_all(projection.parent().expect("projection parent")).expect("mkdir hot dir");
        fs::write(&projection, "# projection\n- stable\n").expect("write projection");

        let mut state = MoonState {
            last_embed_trigger_epoch_secs: Some(321),
            ..MoonState::default()
        };
        state
            .embedded_projection_collections
            .entry(hot_embed_collection_for_session("s3"))
            .or_default()
            .insert(projection.display().to_string(), u64::MAX);
        state
            .pending_embed_collections
            .insert(hot_embed_collection_for_session("s3"), 322);

        let mut input = resolve_input(&paths, None, Some("s3")).expect("resolve input");
        input.embedding_index_anchor =
            Some(embedding_index_anchor_from_state(&paths, &state, "s3"));
        let output = assemble_context(&input).expect("assemble context");

        assert!(output.content.contains("## Embedding Index Anchor"));
        assert!(output.content.contains("- status: pending"));
        assert!(output.content.contains("- hot_collection: history_hot_s3"));
        assert!(output.content.contains("- library_collection: history_lib"));
        assert!(
            output
                .content
                .contains("- pending_embed_collections: history_hot_s3")
        );
        assert!(
            output
                .content
                .contains("- hot_session_projection_indexed: true")
        );
        assert!(output.content.contains(&format!(
            "- library_collection: {}",
            LIBRARY_EMBED_COLLECTION
        )));
    }
}
