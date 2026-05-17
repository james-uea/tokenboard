//! CSV parser for Cursor IDE usage data.
//!
//! Cursor stores token usage as CSV files downloaded from the Cursor API.
//! Format: Timestamp, Model, Input Tokens, Output Tokens, Cache Read Tokens,
//!         Cache Write Tokens, Reasoning Tokens, Cost

use crate::scanner::{ScanFilter, TokenUsage};
use anyhow::{Context, Result};
use chrono::{NaiveDateTime, TimeZone, Utc};
use std::path::Path;

/// Parse a Cursor usage CSV file.
pub fn parse_cursor_csv(path: &Path, filter: Option<&ScanFilter>) -> Result<Vec<TokenUsage>> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_path(path)
        .with_context(|| format!("Cannot read CSV {}", path.display()))?;

    let fallback_ts = file_modified_timestamp(path);
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cursor")
        .to_string();

    let mut usages = Vec::new();

    for result in reader.records() {
        let record = result?;

        // Try multiple column name variants
        let model = find_field(&record, &["Model", "model", "model_name"])
            .unwrap_or("unknown")
            .to_string();

        let input: i64 = find_field(&record, &["Input Tokens", "input_tokens", "InputTokens"])
            .and_then(|s| s.replace(',', "").parse().ok())
            .unwrap_or(0)
            .max(0);

        let output: i64 = find_field(&record, &["Output Tokens", "output_tokens", "OutputTokens"])
            .and_then(|s| s.replace(',', "").parse().ok())
            .unwrap_or(0)
            .max(0);

        let cache_read: i64 = find_field(
            &record,
            &[
                "Cache Read Tokens",
                "cache_read_tokens",
                "CacheReadTokens",
                "Cache Read Input Tokens",
            ],
        )
        .and_then(|s| s.replace(',', "").parse().ok())
        .unwrap_or(0)
        .max(0);

        let cache_write: i64 = find_field(
            &record,
            &[
                "Cache Write Tokens",
                "cache_write_tokens",
                "CacheWriteTokens",
                "Cache Creation Input Tokens",
            ],
        )
        .and_then(|s| s.replace(',', "").parse().ok())
        .unwrap_or(0)
        .max(0);

        let reasoning: i64 = find_field(
            &record,
            &["Reasoning Tokens", "reasoning_tokens", "ReasoningTokens"],
        )
        .and_then(|s| s.replace(',', "").parse().ok())
        .unwrap_or(0)
        .max(0);

        // Skip zero-token rows
        if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 && reasoning == 0 {
            continue;
        }

        let timestamp = find_field(&record, &["Timestamp", "timestamp", "date", "Date"])
            .and_then(|ts| {
                // Try multiple date formats
                NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
                    .or_else(|_| NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S"))
                    .or_else(|_| NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%SZ"))
                    .or_else(|_| NaiveDateTime::parse_from_str(ts, "%Y-%m-%d"))
                    .ok()
            })
            .map(|dt| Utc.from_utc_datetime(&dt).timestamp_millis())
            .unwrap_or(fallback_ts);

        if let Some(f) = filter {
            if !f.include_timestamp(timestamp) {
                continue;
            }
        }

        let provider = if model.to_lowercase().contains("claude") {
            "anthropic"
        } else if model.to_lowercase().contains("gpt") || model.to_lowercase().contains("composer")
        {
            "openai"
        } else if model.to_lowercase().contains("gemini") {
            "google"
        } else {
            "cursor"
        };

        usages.push(TokenUsage {
            agent: "cursor".into(),
            model_id: model,
            provider: provider.into(),
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            reasoning_tokens: reasoning,
            timestamp,
            session_id: session_id.clone(),
            workspace_key: None,
        });
    }

    Ok(usages)
}

/// Find the first matching column in a CSV record.
fn find_field<'a>(record: &'a csv::StringRecord, names: &[&str]) -> Option<&'a str> {
    for name in names {
        if let Ok(idx) = find_header_index(record, name) {
            let val = record.get(idx)?;
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

/// Find a column index by header name (case-insensitive).
fn find_header_index(record: &csv::StringRecord, name: &str) -> Result<usize, ()> {
    let name_lower = name.to_lowercase();
    for (i, header) in record.iter().enumerate() {
        if header.to_lowercase() == name_lower {
            return Ok(i);
        }
    }
    Err(())
}

fn file_modified_timestamp(path: &Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
