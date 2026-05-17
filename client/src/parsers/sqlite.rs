//! SQLite parser for session databases.
//!
//! Handles agents that store usage data in SQLite:
//! Hermes, Kilo, Goose, Crush.

use crate::scanner::{ScanFilter, TokenUsage};
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::Path;

/// Parse a SQLite session database.
pub fn parse_sqlite_db(
    path: &Path,
    agent: &str,
    _filter: Option<&ScanFilter>,
) -> Result<Vec<TokenUsage>> {
    let conn = Connection::open(path)
        .with_context(|| format!("Cannot open SQLite DB {}", path.display()))?;

    match agent {
        "hermes" => parse_hermes(&conn, path),
        "kilo" => parse_kilo(&conn, path),
        "goose" => parse_goose(&conn, path),
        "crush" => parse_crush(&conn, path),
        "opencode" => parse_opencode(&conn, path),
        _ => Ok(Vec::new()),
    }
}

// ============================================================================
// Hermes Agent (state.db)
// ============================================================================

fn parse_hermes(conn: &Connection, path: &Path) -> Result<Vec<TokenUsage>> {
    let fallback_ts = file_modified_timestamp(path);

    // Hermes stores per-session token aggregates in the sessions table.
    // Columns: model, input_tokens, output_tokens, cache_read_tokens,
    //           cache_write_tokens, reasoning_tokens, estimated_cost_usd,
    //           started_at, source, billing_provider
    let query = "SELECT
            COALESCE(s.model, 'hermes') as model,
            COALESCE(s.input_tokens, 0) as input_tokens,
            COALESCE(s.output_tokens, 0) as output_tokens,
            COALESCE(s.cache_read_tokens, 0) as cache_read_tokens,
            COALESCE(s.cache_write_tokens, 0) as cache_write_tokens,
            COALESCE(s.reasoning_tokens, 0) as reasoning_tokens,
            CAST((COALESCE(s.started_at, 0) * 1000) AS INTEGER) as timestamp,
            COALESCE(s.id, 'unknown') as session_id,
            COALESCE(s.billing_provider, 'deepseek') as provider,
            COALESCE(s.source, 'unknown') as source
         FROM sessions s
         WHERE s.input_tokens > 0
            OR s.output_tokens > 0
            OR s.cache_read_tokens > 0
            OR s.cache_write_tokens > 0";

    let mut stmt = conn.prepare(query)?;
    let mut usages = Vec::new();

    let rows = stmt.query_map([], |row| {
        let model: String = row.get(0).unwrap_or_else(|_| "hermes".into());
        let provider: String = row.get(8).unwrap_or_else(|_| "DeepSeek".into());
        let source: String = row.get(9).unwrap_or_else(|_| "hermes".into());
        Ok(TokenUsage {
            agent: "hermes".into(),
            model_id: model,
            provider,
            input_tokens: row.get::<_, i64>(1).unwrap_or(0).max(0),
            output_tokens: row.get::<_, i64>(2).unwrap_or(0).max(0),
            cache_read_tokens: row.get::<_, i64>(3).unwrap_or(0).max(0),
            cache_write_tokens: row.get::<_, i64>(4).unwrap_or(0).max(0),
            reasoning_tokens: row.get::<_, i64>(5).unwrap_or(0).max(0),
            timestamp: row.get::<_, i64>(6).unwrap_or(fallback_ts),
            session_id: row.get::<_, String>(7).unwrap_or_else(|_| "Unknown".into()),
            workspace_key: Some(source),
        })
    })?;

    for row in rows {
        usages.push(row?);
    }

    Ok(usages)
}

// ============================================================================
// Kilo Code (kilo.db)
// ============================================================================

fn parse_kilo(conn: &Connection, path: &Path) -> Result<Vec<TokenUsage>> {
    let fallback_ts = file_modified_timestamp(path);

    let queries = [
        "SELECT
            COALESCE(model, 'kilo') as model,
            COALESCE(input_tokens, 0) as input_tokens,
            COALESCE(output_tokens, 0) as output_tokens,
            COALESCE(cache_read_tokens, 0) as cache_read_tokens,
            COALESCE(cache_write_tokens, 0) as cache_write_tokens,
            COALESCE(reasoning_tokens, 0) as reasoning_tokens,
            COALESCE(timestamp, 0) as timestamp,
            COALESCE(session_id, 'unknown') as session_id
         FROM messages
         WHERE input_tokens > 0 OR output_tokens > 0",
        "SELECT
            COALESCE(model, 'kilo') as model,
            COALESCE(input_tokens, 0),
            COALESCE(output_tokens, 0),
            0, 0, 0,
            COALESCE(created_at, 0),
            COALESCE(session_id, 'unknown')
         FROM turns
         WHERE (input_tokens > 0 OR output_tokens > 0)",
    ];

    for query in &queries {
        if let Ok(mut stmt) = conn.prepare(query) {
            let mut usages = Vec::new();
            let rows = stmt.query_map([], |row| {
                Ok(TokenUsage {
                    agent: "kilo".into(),
                    model_id: row.get::<_, String>(0).unwrap_or_else(|_| "kilo".into()),
                    provider: infer_provider_from_model(
                        &row.get::<_, String>(0).unwrap_or_default(),
                    ),
                    input_tokens: row.get::<_, i64>(1).unwrap_or(0).max(0),
                    output_tokens: row.get::<_, i64>(2).unwrap_or(0).max(0),
                    cache_read_tokens: row.get::<_, i64>(3).unwrap_or(0).max(0),
                    cache_write_tokens: row.get::<_, i64>(4).unwrap_or(0).max(0),
                    reasoning_tokens: row.get::<_, i64>(5).unwrap_or(0).max(0),
                    timestamp: row.get::<_, i64>(6).unwrap_or(fallback_ts),
                    session_id: row.get::<_, String>(7).unwrap_or_else(|_| "Unknown".into()),
                    workspace_key: None,
                })
            });

            if let Ok(rows) = rows {
                for usage in rows.flatten() {
                    usages.push(usage);
                }
            }

            if !usages.is_empty() {
                return Ok(usages);
            }
        }
    }

    Ok(Vec::new())
}

// ============================================================================
// Goose (sessions.db)
// ============================================================================

fn parse_goose(conn: &Connection, path: &Path) -> Result<Vec<TokenUsage>> {
    let fallback_ts = file_modified_timestamp(path);

    let queries = [
        "SELECT
            COALESCE(model, 'goose') as model,
            COALESCE(input_tokens, 0) as input_tokens,
            COALESCE(output_tokens, 0) as output_tokens,
            COALESCE(cache_read_tokens, 0) as cache_read_tokens,
            COALESCE(cache_write_tokens, 0) as cache_write_tokens,
            COALESCE(reasoning_tokens, 0) as reasoning_tokens,
            COALESCE(timestamp, 0) as timestamp,
            COALESCE(session_id, 'unknown') as session_id
         FROM messages
         WHERE (input_tokens > 0 OR output_tokens > 0)
           AND role = 'assistant'",
        "SELECT
            COALESCE(model, 'goose') as model,
            COALESCE(input_tokens, 0),
            COALESCE(output_tokens, 0),
            0, 0, 0,
            COALESCE(created_at, 0),
            COALESCE(session_id, 'unknown')
         FROM session_messages
         WHERE role = 'assistant'
           AND (input_tokens > 0 OR output_tokens > 0)",
    ];

    for query in &queries {
        if let Ok(mut stmt) = conn.prepare(query) {
            let mut usages = Vec::new();
            let rows = stmt.query_map([], |row| {
                Ok(TokenUsage {
                    agent: "goose".into(),
                    model_id: row.get::<_, String>(0).unwrap_or_else(|_| "goose".into()),
                    provider: infer_provider_from_model(
                        &row.get::<_, String>(0).unwrap_or_default(),
                    ),
                    input_tokens: row.get::<_, i64>(1).unwrap_or(0).max(0),
                    output_tokens: row.get::<_, i64>(2).unwrap_or(0).max(0),
                    cache_read_tokens: row.get::<_, i64>(3).unwrap_or(0).max(0),
                    cache_write_tokens: row.get::<_, i64>(4).unwrap_or(0).max(0),
                    reasoning_tokens: row.get::<_, i64>(5).unwrap_or(0).max(0),
                    timestamp: row.get::<_, i64>(6).unwrap_or(fallback_ts),
                    session_id: row.get::<_, String>(7).unwrap_or_else(|_| "Unknown".into()),
                    workspace_key: None,
                })
            });

            if let Ok(rows) = rows {
                for usage in rows.flatten() {
                    usages.push(usage);
                }
            }

            if !usages.is_empty() {
                return Ok(usages);
            }
        }
    }

    Ok(Vec::new())
}

// ============================================================================
// Crush (crush.db)
// ============================================================================

fn parse_crush(conn: &Connection, path: &Path) -> Result<Vec<TokenUsage>> {
    let fallback_ts = file_modified_timestamp(path);

    let queries = [
        "SELECT
            COALESCE(model, 'crush') as model,
            COALESCE(input_tokens, 0) as input_tokens,
            COALESCE(output_tokens, 0) as output_tokens,
            COALESCE(cache_read_tokens, 0) as cache_read_tokens,
            COALESCE(cache_write_tokens, 0) as cache_write_tokens,
            COALESCE(reasoning_tokens, 0) as reasoning_tokens,
            COALESCE(timestamp, 0) as timestamp,
            COALESCE(session_id, 'unknown') as session_id
         FROM messages
         WHERE role = 'assistant'
           AND (input_tokens > 0 OR output_tokens > 0)",
        "SELECT
            COALESCE(model, 'crush') as model,
            COALESCE(input_tokens, 0),
            COALESCE(output_tokens, 0),
            0, 0, 0,
            COALESCE(created_at, 0),
            'unknown'
         FROM turns
         WHERE (input_tokens > 0 OR output_tokens > 0)",
    ];

    for query in &queries {
        if let Ok(mut stmt) = conn.prepare(query) {
            let mut usages = Vec::new();
            let rows = stmt.query_map([], |row| {
                Ok(TokenUsage {
                    agent: "crush".into(),
                    model_id: row.get::<_, String>(0).unwrap_or_else(|_| "crush".into()),
                    provider: infer_provider_from_model(
                        &row.get::<_, String>(0).unwrap_or_default(),
                    ),
                    input_tokens: row.get::<_, i64>(1).unwrap_or(0).max(0),
                    output_tokens: row.get::<_, i64>(2).unwrap_or(0).max(0),
                    cache_read_tokens: row.get::<_, i64>(3).unwrap_or(0).max(0),
                    cache_write_tokens: row.get::<_, i64>(4).unwrap_or(0).max(0),
                    reasoning_tokens: row.get::<_, i64>(5).unwrap_or(0).max(0),
                    timestamp: row.get::<_, i64>(6).unwrap_or(fallback_ts),
                    session_id: row.get::<_, String>(7).unwrap_or_else(|_| "Unknown".into()),
                    workspace_key: None,
                })
            });

            if let Ok(rows) = rows {
                for usage in rows.flatten() {
                    usages.push(usage);
                }
            }

            if !usages.is_empty() {
                return Ok(usages);
            }
        }
    }

    Ok(Vec::new())
}

// ============================================================================
// OpenCode (opencode.db)
// ============================================================================

fn parse_opencode(conn: &Connection, path: &Path) -> Result<Vec<TokenUsage>> {
    let fallback_ts = file_modified_timestamp(path);

    let query = "SELECT data FROM message WHERE data LIKE '%\"modelID\"%' AND data NOT LIKE '%\"modelID\":null%'";

    let mut stmt = conn.prepare(query)?;
    let mut usages = Vec::new();

    let rows = stmt.query_map([], |row| {
        let data: String = row.get(0).unwrap_or_default();
        Ok(data)
    })?;

    for row in rows {
        let data = row?;
        let value: serde_json::Value = match serde_json::from_str(&data) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let role = value.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role != "assistant" {
            continue;
        }

        let model = value
            .get("modelID")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let provider = value
            .get("providerID")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let tokens = match value.get("tokens") {
            Some(t) => t,
            None => continue,
        };

        let input = tokens
            .get("input")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0);
        let output = tokens
            .get("output")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0);
        let reasoning = tokens
            .get("reasoning")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0);

        let cache = tokens.get("cache");
        let cache_read = cache
            .and_then(|c| c.get("read"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0);
        let cache_write = cache
            .and_then(|c| c.get("write"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            .max(0);

        let agent_name = value
            .get("agent")
            .and_then(|v| v.as_str())
            .unwrap_or("opencode")
            .to_string();

        let timestamp = value
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(|v| v.as_f64())
            .map(|ts| ts as i64)
            .unwrap_or(fallback_ts);

        usages.push(TokenUsage {
            agent: "opencode".into(),
            model_id: model,
            provider,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            reasoning_tokens: reasoning,
            timestamp,
            session_id: agent_name,
            workspace_key: None,
        });
    }

    Ok(usages)
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

fn infer_provider_from_model(model: &str) -> String {
    let lower = model.to_lowercase();
    if lower.contains("claude") {
        return "Anthropic".into();
    }
    if lower.contains("gpt") || lower.contains("o1") || lower.contains("o3") || lower.contains("o4")
    {
        return "OpenAI".into();
    }
    if lower.contains("gemini") {
        return "Google".into();
    }
    if lower.contains("llama") || lower.contains("mistral") {
        return "Meta".into();
    }
    if lower.contains("qwen") {
        return "Alibaba".into();
    }
    if lower.contains("deepseek") {
        return "DeepSeek".into();
    }
    if lower.contains("hermes") {
        return "Nous".into();
    }
    "Unknown".into()
}
