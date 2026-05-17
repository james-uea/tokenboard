//! Generic JSON parser for single-file clients.
//!
//! Handles agents that store one JSON object or array per file:
//! OpenCode, Amp, Droid, RooCode, KiloCode, Mux, Codebuff.

use crate::scanner::{ScanFilter, TokenUsage};
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::path::Path;

/// Parse a JSON session file and extract token usage records.
pub fn parse_json_file(
    path: &Path,
    agent: &str,
    filter: Option<&ScanFilter>,
) -> Result<Vec<TokenUsage>> {
    let content =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Invalid JSON in {}", path.display()))?;

    let fallback_ts = file_modified_timestamp(path);
    let session_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .or_else(|| path.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("Unknown")
        .to_string();

    let mut usages = Vec::new();

    // Some clients store an array of messages, others a single object
    if let Some(array) = value.as_array() {
        for item in array {
            if let Some(usage) = extract_json(agent, item, &session_id, fallback_ts) {
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
    } else if let Some(usage) = extract_json(agent, &value, &session_id, fallback_ts) {
        if usage.input_tokens > 0
            || usage.output_tokens > 0
            || usage.cache_read_tokens > 0
            || usage.cache_write_tokens > 0
            || usage.reasoning_tokens > 0
        {
            if let Some(f) = filter {
                if f.include_timestamp(usage.timestamp) {
                    usages.push(usage);
                }
            } else {
                usages.push(usage);
            }
        }
    }

    Ok(usages)
}

fn extract_json(
    agent: &str,
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    match agent {
        "opencode" => extract_opencode(value, session_id, fallback_ts),
        "amp" => extract_amp(value, session_id, fallback_ts),
        "droid" => extract_droid(value, session_id, fallback_ts),
        "roocode" | "kilocode" => extract_vscode_agent(value, session_id, fallback_ts, agent),
        "mux" => extract_mux(value, session_id, fallback_ts),
        "codebuff" => extract_codebuff(value, session_id, fallback_ts),
        _ => None,
    }
}

// ============================================================================
// OpenCode message extractor
// ============================================================================

fn extract_opencode(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // OpenCode messages have: { role: "assistant", usage: { input_tokens, output_tokens }, model }
    let role = value.get("role")?.as_str()?;
    if role != "assistant" {
        return None;
    }
    let usage = value.get("usage")?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
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
    let reasoning = usage
        .get("reasoning_tokens")
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
        .or_else(|| {
            value
                .get("model")
                .and_then(|v| v.as_str())
                .map(infer_provider)
        })
        .unwrap_or("Unknown");

    Some(TokenUsage {
        agent: "opencode".into(),
        model_id: model,
        provider: provider.into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: reasoning,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Amp thread extractor
// ============================================================================

fn extract_amp(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Amp stores messages with usage in the message object
    let usage = value.get("usage")?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("model_name").and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
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

    Some(TokenUsage {
        agent: "amp".into(),
        model_id: model,
        provider: infer_provider_from_model(value).into(),
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
// Factory Droid settings extractor
// ============================================================================

fn extract_droid(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Droid stores usage in a nested stats object
    let stats = value.get("stats").or_else(|| value.get("usage"))?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| stats.get("model").and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
        .to_string();

    let input = stats
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = stats
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("last_updated").and_then(|v| v.as_str()))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    Some(TokenUsage {
        agent: "droid".into(),
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
// VS Code agent extractor (RooCode, KiloCode)
// ============================================================================

fn extract_vscode_agent(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
    agent: &str,
) -> Option<TokenUsage> {
    // RooCode/KiloCode ui_messages.json stores messages with usage in metadata
    // Each message is { role, text, metadata?: { usage?: { inputTokens, outputTokens } } }
    let metadata = value.get("metadata").or_else(|| value.get("meta"))?;

    // Try nested usage paths
    let usage = metadata
        .get("usage")
        .or_else(|| value.get("usage"))
        .or_else(|| metadata.get("apiMetrics").and_then(|m| m.get("usage")))?;

    let model = metadata
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("model").and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
        .to_string();

    let input = usage
        .get("inputTokens")
        .or_else(|| usage.get("input_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = usage
        .get("outputTokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_read = usage
        .get("cacheReadTokens")
        .or_else(|| usage.get("cache_read_input_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = {
        let ts_str = value
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| {
                metadata
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .or_else(|| {
                metadata.get("created").and_then(|v| v.as_f64()).map(|ts| {
                    let secs = ts as i64;
                    Utc.timestamp_opt(secs, 0)
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
        agent: agent.into(),
        model_id: model,
        provider: infer_provider_from_model(value).into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Mux session-usage extractor
// ============================================================================

fn extract_mux(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Mux session-usage.json is a direct usage summary
    // { model: "...", input_tokens: N, output_tokens: N, cost: N.N }
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown")
        .to_string();

    let input = value
        .get("input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let output = value
        .get("output_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_read = value
        .get("cache_read_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);
    let cache_write = value
        .get("cache_creation_input_tokens")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0);

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("date").and_then(|v| v.as_str()))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(fallback_ts);

    Some(TokenUsage {
        agent: "mux".into(),
        model_id: model,
        provider: infer_provider_from_model(value).into(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: 0,
        timestamp,
        session_id: session_id.into(),
        workspace_key: None,
    })
}

// ============================================================================
// Codebuff chat-messages extractor
// ============================================================================

fn extract_codebuff(
    value: &serde_json::Value,
    session_id: &str,
    fallback_ts: i64,
) -> Option<TokenUsage> {
    // Codebuff stores chat messages with usage in message.usage
    let role = value.get("role").or_else(|| value.get("type"))?.as_str()?;
    if role != "assistant" && role != "ai" {
        return None;
    }
    let usage = value.get("usage")?;
    let model = value
        .get("model")
        .and_then(|v| v.as_str())
        .or_else(|| value.get("model_name").and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
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

    Some(TokenUsage {
        agent: "codebuff".into(),
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
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Guess provider from a model name prefix.
fn infer_provider(model: &str) -> &str {
    let lower = model.to_lowercase();
    if lower.contains("claude") {
        return "Anthropic";
    }
    if lower.contains("gpt") || lower.contains("o1") || lower.contains("o3") || lower.contains("o4")
    {
        return "OpenAI";
    }
    if lower.contains("gemini") {
        return "Google";
    }
    if lower.contains("llama") || lower.contains("mistral") || lower.contains("mixtral") {
        return "Meta";
    }
    if lower.contains("qwen") {
        return "Alibaba";
    }
    if lower.contains("kimi") || lower.contains("moonshot") {
        return "moonshot";
    }
    if lower.contains("deepseek") {
        return "DeepSeek";
    }
    "Unknown"
}

fn infer_provider_from_model(value: &serde_json::Value) -> &str {
    value
        .get("model")
        .and_then(|v| v.as_str())
        .map(infer_provider)
        .or_else(|| value.get("provider").and_then(|v| v.as_str()))
        .unwrap_or("Unknown")
}
