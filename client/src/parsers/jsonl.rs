//! Generic JSONL parser with per-client extractors.
//!
//! Each AI coding agent stores session data in JSONL files with different
//! schemas. This module provides a single `parse_jsonl_file()` entry point
//! that dispatches to the right extractor based on the agent id.

use crate::scanner::{ScanFilter, TokenUsage};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

// ============================================================================
// Core JSONL parsing
// ============================================================================

/// Parse a JSONL file and extract token usage records.
///
/// `agent` identifies which extractor to use (e.g. `"gemini"`, `"copilot"`).
pub fn parse_jsonl_file(
    path: &Path,
    agent: &str,
    filter: Option<&ScanFilter>,
    mut global_dedup: Option<&mut HashSet<String>>,
) -> Result<Vec<TokenUsage>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let reader = BufReader::new(file);
    let fallback_ts = file_modified_timestamp(path);
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut usages = Vec::new();
    // Dedup map for Claude streaming duplicates: dedup_key → index in usages
    let mut claude_dedup: HashMap<String, usize> = HashMap::new();

    // Gemini stores older/exported sessions as a single JSON document and
    // current sessions as JSONL. Try the full document first, then JSONL.
    if agent == "gemini" {
        let mut file =
            std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        if contents.trim().is_empty() {
            return Ok(usages);
        }

        let mut raw_entries: Vec<serde_json::Value> = Vec::new();
        if let Ok(root) = serde_json::from_str::<serde_json::Value>(&contents) {
            collect_gemini_entries(&root, &mut raw_entries);
        } else {
            for (line_idx, line) in contents.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let value: serde_json::Value =
                    serde_json::from_str(trimmed).with_context(|| {
                        format!(
                            "Invalid Gemini JSONL in {} at line {}",
                            path.display(),
                            line_idx + 1
                        )
                    })?;
                collect_gemini_entries(&value, &mut raw_entries);
            }
        };

        let fallback_ts = file_modified_timestamp(path);
        for value in &raw_entries {
            if let Some(usage) = extract_jsonl(agent, value, &session_id, fallback_ts, path) {
                if let Some(f) = filter {
                    if !f.include_timestamp(usage.timestamp) {
                        continue;
                    }
                }
                if usage.input_tokens == 0
                    && usage.output_tokens == 0
                    && usage.cache_read_tokens == 0
                    && usage.cache_write_tokens == 0
                    && usage.reasoning_tokens == 0
                {
                    continue;
                }
                usages.push(usage);
            }
        }
        return Ok(usages);
    }

    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: serde_json::Value = serde_json::from_str(trimmed)?;

        // Claude-specific dedup: streaming responses produce duplicate entries
        // with progressively larger token counts. Dedup using messageId:requestId
        // composite key and merge via per-field max.
        if agent == "claude" {
            if let Some(dedup_key) = claude_dedup_key(&value) {
                if let Some(&existing_idx) = claude_dedup.get(&dedup_key) {
                    // Merge: take per-field max of token counts
                    if let Some(usage) =
                        extract_claude_dedup_merge(&value, &session_id, fallback_ts, path)
                    {
                        let existing: &mut TokenUsage = &mut usages[existing_idx];
                        existing.input_tokens = existing.input_tokens.max(usage.input_tokens);
                        existing.output_tokens = existing.output_tokens.max(usage.output_tokens);
                        existing.cache_read_tokens =
                            existing.cache_read_tokens.max(usage.cache_read_tokens);
                        existing.cache_write_tokens =
                            existing.cache_write_tokens.max(usage.cache_write_tokens);
                        existing.reasoning_tokens =
                            existing.reasoning_tokens.max(usage.reasoning_tokens);
                    }
                    continue;
                }
            }
        }

        if let Some(usage) = extract_jsonl(agent, &value, &session_id, fallback_ts, path) {
            if let Some(f) = filter {
                if !f.include_timestamp(usage.timestamp) {
                    continue;
                }
            }
            // Skip zero-token entries
            if usage.input_tokens == 0
                && usage.output_tokens == 0
                && usage.cache_read_tokens == 0
                && usage.cache_write_tokens == 0
                && usage.reasoning_tokens == 0
            {
                continue;
            }

            // Record dedup key AFTER all checks pass.
            // Global cross-file dedup: skip entries whose dedup key
            // was already seen in another file (subagent sidechains
            // duplicate parent-session request pairs).
            if agent == "claude" {
                if let Some(dedup_key) = claude_dedup_key(&value) {
                    if let Some(ref mut global) = global_dedup {
                        if !global.insert(dedup_key.clone()) {
                            continue; // cross-file duplicate — skip
                        }
                    }
                    claude_dedup.insert(dedup_key, usages.len());
                }
            }

            usages.push(usage);
        }
    }

    Ok(usages)
}

fn collect_gemini_entries(value: &serde_json::Value, entries: &mut Vec<serde_json::Value>) {
    if let Some(array) = value.as_array() {
        for item in array {
            collect_gemini_entries(item, entries);
        }
    } else if let Some(messages) = value.get("messages").and_then(|v| v.as_array()) {
        entries.extend(messages.iter().cloned());
    } else {
        entries.push(value.clone());
    }
}

// ============================================================================
// Agent dispatch
// ============================================================================

fn extract_jsonl(
    agent: &str,
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    path: &Path,
) -> Option<TokenUsage> {
    match agent {
        "claude" => extract_claude(value, session_id, fallback_ts, path),
        "gemini" => extract_gemini(value, session_id, fallback_ts),
        "openclaw" => extract_openclaw(value, session_id, fallback_ts),
        "pi" => extract_pi(value, session_id, fallback_ts, path),
        "kimi" => extract_kimi(value, session_id, fallback_ts),
        "qwen" => extract_qwen(value, session_id, fallback_ts, path),
        "copilot" => extract_copilot(value, session_id, fallback_ts),
        "antigravity" => extract_antigravity(value, session_id, fallback_ts),
        _ => None,
    }
}

// ============================================================================
// Claude Code extractor (ported from scanner.rs)
// ============================================================================

/// Build a dedup key for Claude Code entries: "messageId:requestId".
/// Returns None if either field is missing (not a dedup-able entry).
fn claude_dedup_key(value: &serde_json::Value) -> Option<String> {
    let msg_id = value
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str())?;
    let req_id = value.get("requestId").and_then(|v| v.as_str())?;
    Some(format!("{}:{}", msg_id, req_id))
}

/// Extract just the token fields from a Claude entry for merge purposes.
/// This runs a lightweight extraction without workspace_key computation.
fn extract_claude_dedup_merge(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    path: &Path,
) -> Option<TokenUsage> {
    if value.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message.get("model")?.as_str()?.to_string();

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    let workspace_key = extract_claude_workspace_key(path);

    Some(TokenUsage {
        agent: "claude".into(),
        model_id: model,
        provider: "Anthropic".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key,
    })
}

fn extract_claude(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    path: &Path,
) -> Option<TokenUsage> {
    // Only assistant messages with usage
    if value.get("type")?.as_str()? != "assistant" {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message.get("model")?.as_str()?.to_string();

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_write = usage
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let workspace_key = extract_claude_workspace_key(path);

    Some(TokenUsage {
        agent: "claude".into(),
        model_id: model,
        provider: "Anthropic".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key,
    })
}

fn extract_claude_workspace_key(path: &Path) -> Option<String> {
    let components: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();

    for window in components.windows(3) {
        if window[0] == ".claude" && window[1] == "projects" {
            let raw = &window[2];
            let decoded = raw.strip_prefix('-').unwrap_or(raw);
            return Some(decoded.to_string());
        }
    }
    None
}

// ============================================================================
// Gemini CLI extractor
// ============================================================================

fn extract_gemini(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Session wrappers with `messages` array are unwrapped by the reader.
    // Here we handle individual message entries with: type, model, tokens.
    let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if msg_type != "gemini" {
        return None;
    }

    let tokens = value.get("tokens")?;

    let raw_input = tokens
        .get("input")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = tokens
        .get("output")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cached = tokens
        .get("cached")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let thoughts = tokens
        .get("thoughts")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    // Gemini reports cache-inclusive input — subtract the cache portion
    // so we don't double-count cached tokens in both input and cache_read.
    let cache_portion = cached.min(raw_input);
    let input = raw_input.saturating_sub(cache_portion);

    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gemini")
        .to_string();

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    Some(TokenUsage {
        agent: "gemini".into(),
        model_id: model,
        provider: "Google".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        reasoning_tokens: thoughts,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// OpenClaw extractor
// ============================================================================

fn extract_openclaw(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // OpenClaw stores OpenAI-format usage in a nested `usage` object
    let usage = value.get("usage")?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|r| r.get("model"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("unknown")
        .to_string();

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("created_at").and_then(|v| v.as_str()))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    let provider = value
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("openai")
        .to_string();

    // Detect cache tokens from usage_details if present
    let cache_read = usage
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_i64())
        .or_else(|| {
            usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_i64())
        })
        .unwrap_or(0)
        .max(0);

    let reasoning = usage
        .get("output_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    Some(TokenUsage {
        agent: "openclaw".into(),
        model_id: model,
        provider,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
        reasoning_tokens: reasoning,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Pi AI extractor
// ============================================================================

fn extract_pi(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    path: &Path,
) -> Option<TokenUsage> {
    // Pi stores usage in a top-level `usage` object
    let usage = value.get("usage").or_else(|| value.get("token_usage"))?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("model_name").and_then(|v| v.as_str()))
        .unwrap_or("unknown")
        .to_string();

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    let workspace_key = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    Some(TokenUsage {
        agent: "pi".into(),
        model_id: model,
        provider: "Anthropic".into(), // Pi uses Claude models
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key,
    })
}

// ============================================================================
// Kimi extractor
// ============================================================================

fn extract_kimi(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Kimi wire.jsonl contains API request/response pairs
    let response = value.get("response")?;
    let usage = response.get("usage")?;
    let model = response
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("model").and_then(|v| v.as_str()))
        .unwrap_or("moonshot")
        .to_string();

    let input = usage
        .get("prompt_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("completion_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = {
        let ts_str = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                response.get("created").and_then(|v| v.as_i64()).map(|ts| {
                    Utc.timestamp_opt(ts, 0)
                        .single()
                        .unwrap_or_else(Utc::now)
                        .to_rfc3339()
                })
            });
        ts_str
            .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(fallback_ts)
    };

    Some(TokenUsage {
        agent: "kimi".into(),
        model_id: model,
        provider: "Moonshot".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Qwen Code extractor
// ============================================================================

fn extract_qwen(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    path: &Path,
) -> Option<TokenUsage> {
    // Qwen Code stores sessions similar to Claude Code (jsonl per session)
    // Entries have type: "assistant" with message.usage
    let entry_type = value.get("type")?.as_str()?;
    if entry_type != "assistant" {
        return None;
    }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("qwen")
        .to_string();

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let workspace_key = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string());

    Some(TokenUsage {
        agent: "qwen".into(),
        model_id: model,
        provider: "Alibaba".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key,
    })
}

// ============================================================================
// GitHub Copilot extractor
// ============================================================================

fn extract_copilot(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Copilot OTEL traces contain token usage in span attributes
    // Look for spans with llm.usage attributes
    let usage = value
        .get("usage")
        .or_else(|| value.get("token_usage"))
        .or_else(|| value.get("attributes").and_then(|a| a.get("llm.usage")))?;

    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("attributes")
                .and_then(|a| a.get("llm.model"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("copilot")
        .to_string();

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("timeUnixNano").and_then(|v| v.as_str()))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    Some(TokenUsage {
        agent: "copilot".into(),
        model_id: model,
        provider: "GitHub".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Antigravity extractor
// ============================================================================

fn extract_antigravity(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Antigravity caches API responses as JSONL
    let usage = value.get("usage")?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| {
            value
                .get("response")
                .and_then(|r| r.get("model"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("unknown")
        .to_string();

    let input = usage
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = {
        let ts_str = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        ts_str
            .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
            .map(|dt| dt.timestamp_millis())
            .unwrap_or(fallback_ts)
    };

    Some(TokenUsage {
        agent: "antigravity".into(),
        model_id: model,
        provider: "Anthropic".into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Helpers
// ============================================================================

fn file_modified_timestamp(path: &Path) -> i64 {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gemini_jsonl_session_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session-2026-05-18T18-01-06b26b6f.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"metadata","sessionId":"ignored"}"#,
                "\n",
                r#"{"type":"gemini","model":"gemini-2.5-pro","timestamp":"2026-05-18T18:01:06Z","tokens":{"input":120,"output":45,"cached":20,"thoughts":7}}"#,
                "\n",
                r#"{"type":"gemini","model":"gemini-2.5-flash","tokens":{"input":10,"output":5}}"#,
                "\n"
            ),
        )
        .unwrap();

        let usages = parse_jsonl_file(&path, "gemini", None, None).unwrap();

        assert_eq!(usages.len(), 2);
        assert_eq!(usages[0].model_id, "gemini-2.5-pro");
        assert_eq!(usages[0].input_tokens, 100);
        assert_eq!(usages[0].cache_read_tokens, 20);
        assert_eq!(usages[0].output_tokens, 45);
        assert_eq!(usages[0].reasoning_tokens, 7);
        assert_eq!(usages[0].timestamp, 1_779_127_266_000);
        assert_eq!(usages[1].model_id, "gemini-2.5-flash");
        assert_eq!(usages[1].input_tokens, 10);
    }

    #[test]
    fn still_parses_gemini_json_session_wrappers() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("session-2026-05-18T18-01-06b26b6f.json");
        std::fs::write(
            &path,
            r#"{"sessionId":"abc","messages":[{"type":"user","content":"hello"},{"type":"gemini","model":"gemini-2.5-pro","tokens":{"input":80,"output":30,"cached":10}}]}"#,
        )
        .unwrap();

        let usages = parse_jsonl_file(&path, "gemini", None, None).unwrap();

        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].model_id, "gemini-2.5-pro");
        assert_eq!(usages[0].input_tokens, 70);
        assert_eq!(usages[0].cache_read_tokens, 10);
        assert_eq!(usages[0].output_tokens, 30);
    }
}
