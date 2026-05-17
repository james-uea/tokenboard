//! Scanner for AI coding agent session data.
//!
//! Discovers session files from 21 supported agents via the client registry,
//! dispatches to per-format parsers (JSONL, JSON, CSV, SQLite), and aggregates
//! results into daily summaries for submission.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::clients::{ClientDef, ParserKind, CLIENTS};
use crate::parsers::{csv, json, jsonl, sqlite};
use crate::pricing::{CostTokens, PricingService};

// ============================================================================
// Types
// ============================================================================

/// Filter applied during scanning.
#[derive(Debug, Clone, Default)]
pub struct ScanFilter {
    pub clients: Option<Vec<String>>,
    pub since_ms: Option<i64>,
    pub until_ms: Option<i64>,
}

impl ScanFilter {
    pub fn include_agent(&self, agent: &str) -> bool {
        match &self.clients {
            Some(list) => list.iter().any(|c| c.eq_ignore_ascii_case(agent)),
            None => true,
        }
    }

    pub fn include_timestamp(&self, ts_ms: i64) -> bool {
        if let Some(s) = self.since_ms {
            if ts_ms < s {
                return false;
            }
        }
        if let Some(u) = self.until_ms {
            if ts_ms > u {
                return false;
            }
        }
        true
    }
}

/// Represents a single token usage record from a session file.
#[derive(Debug, Clone)]
pub struct TokenUsage {
    pub agent: String,
    pub model_id: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub timestamp: i64,
    pub session_id: String,
    pub workspace_key: Option<String>,
}

/// Aggregated token data for a single day.
#[derive(Debug, Clone, Default)]
pub struct DailyAggregate {
    pub total_tokens: i64,
    pub total_cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub models: HashMap<String, ModelAggregate>,
    pub clients: HashMap<String, ClientAggregate>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelAggregate {
    pub tokens: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_cost: f64,
    pub provider: String,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClientAggregate {
    pub tokens: i64,
    pub cost: f64,
}

// ============================================================================
// Generic file discovery
// ============================================================================

/// Scan all registered client directories.
pub fn scan_all(filter: Option<&ScanFilter>) -> Result<Vec<TokenUsage>> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    let home_str = home.to_string_lossy();

    let mut all_usages = Vec::new();
    // Global dedup set for Claude — messageId:requestId keys seen across all files
    let mut claude_global_dedup: HashSet<String> = HashSet::new();
    // Global dedup set for Codex token_count snapshots seen across sessions/archives
    let mut codex_global_dedup: HashSet<String> = HashSet::new();

    for client_def in CLIENTS {
        if let Some(f) = filter {
            if !f.include_agent(client_def.id) {
                continue;
            }
        }

        let root = client_def.resolve_path(&home_str);
        let root_path = Path::new(&root);
        if !root_path.exists() && client_def.id != "codex" {
            log::debug!("{}: directory not found ({})", client_def.id, root);
            continue;
        }

        // SQLite clients resolve to a single file; Codex also has archived sessions.
        let files: Vec<std::path::PathBuf> = if client_def.id == "codex" {
            discover_codex_files(root_path, client_def.pattern)
        } else if root_path.is_file() {
            vec![root_path.to_path_buf()]
        } else {
            discover_files(root_path, client_def.pattern)
        };
        log::debug!(
            "{} ({}; default={}): found {} candidate files",
            client_def.id,
            client_def.label,
            client_def.submit_default,
            files.len()
        );

        for file_path in &files {
            let gd: Option<&mut HashSet<String>> = match client_def.id {
                "claude" => Some(&mut claude_global_dedup),
                "codex" => Some(&mut codex_global_dedup),
                _ => None,
            };
            let result = scan_file(file_path, client_def, filter, gd);
            match result {
                Ok(usages) => all_usages.extend(usages),
                Err(e) => {
                    log::warn!(
                        "{}: failed to parse {}: {}",
                        client_def.id,
                        file_path.display(),
                        e
                    );
                }
            }
        }
    }

    // Normalize model names: strip date suffixes from Claude models
    // e.g. "claude-haiku-4-5-20251001" → "claude-haiku-4-5"
    for usage in &mut all_usages {
        if let Some(normalized) = normalize_model_name(&usage.model_id) {
            usage.model_id = normalized;
        }
        // Capitalize first letter of source/agent name
        usage.agent = capitalize_first(&usage.agent);
        // Normalize provider casing
        usage.provider = normalize_provider(&usage.provider);
    }

    all_usages.sort_by_key(|u| u.timestamp);
    log::info!(
        "Scan complete: {} records from {} agents",
        all_usages.len(),
        all_usages
            .iter()
            .map(|u| u.agent.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len()
    );
    Ok(all_usages)
}

/// Discover files matching a pattern under a root directory.
fn discover_files(root: &Path, pattern: &str) -> Vec<PathBuf> {
    use walkdir::WalkDir;

    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            if !e.file_type().is_file() {
                return false;
            }
            let name = e.file_name().to_string_lossy();
            match_pattern(&name, pattern)
        })
        .map(|e| e.path().to_path_buf())
        .collect();
    files.sort_unstable();
    files
}

/// Discover Codex session files from active and archived roots.
fn discover_codex_files(sessions_root: &Path, pattern: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if sessions_root.exists() {
        if sessions_root.is_file() {
            files.push(sessions_root.to_path_buf());
        } else {
            files.extend(discover_files(sessions_root, pattern));
        }
    }

    if let Some(codex_home) = sessions_root.parent() {
        let archived_root = codex_home.join("archived_sessions");
        if archived_root.exists() {
            files.extend(discover_files(&archived_root, pattern));
        }
    }

    files.sort_unstable();
    files.dedup();
    files
}

/// Match a filename against a pattern string.
/// Supports: `*.jsonl`, `*.json`, `*.json|*.jsonl`, `usage*.csv`, `T-*.json`, etc.
fn match_pattern(name: &str, pattern: &str) -> bool {
    if pattern.contains('|') {
        return pattern.split('|').any(|p| match_single(name, p.trim()));
    }
    match_single(name, pattern)
}

fn match_single(name: &str, pattern: &str) -> bool {
    // Exact match
    if !pattern.contains('*') {
        return name == pattern;
    }
    // Handle prefix*suffix patterns (e.g., "usage*.csv", "T-*.json")
    if let Some((prefix, suffix)) = pattern.split_once('*') {
        if prefix.is_empty() && suffix.is_empty() {
            return true; // "*" matches everything
        }
        if prefix.is_empty() {
            return name.ends_with(suffix);
        }
        if suffix.is_empty() {
            return name.starts_with(prefix);
        }
        // Both prefix and suffix: must match both
        return name.starts_with(prefix) && name.ends_with(suffix);
    }
    false
}

// ============================================================================
// File-level dispatch
// ============================================================================

/// Scan a single file using the appropriate parser for its client.
fn scan_file(
    path: &Path,
    client: &ClientDef,
    filter: Option<&ScanFilter>,
    global_dedup: Option<&mut HashSet<String>>,
) -> Result<Vec<TokenUsage>> {
    match client.parser {
        ParserKind::Jsonl => {
            // Codex needs special incremental tracking — handle separately
            if client.id == "codex" {
                scan_codex_file(path, filter, global_dedup)
            } else {
                jsonl::parse_jsonl_file(path, client.id, filter, global_dedup)
            }
        }
        ParserKind::Json => json::parse_json_file(path, client.id, filter),
        ParserKind::Csv => csv::parse_cursor_csv(path, filter),
        ParserKind::Sqlite => sqlite::parse_sqlite_db(path, client.id, filter),
    }
}

// ============================================================================
// Codex parser — kept standalone due to complex incremental token tracking
// ============================================================================

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct CodexEntry {
    #[serde(rename = "type")]
    entry_type: String,
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Debug, Deserialize)]
struct CodexPayload {
    id: Option<String>,
    forked_from_id: Option<String>,
    #[serde(rename = "type")]
    payload_type: Option<String>,
    model: Option<String>,
    model_name: Option<String>,
    model_info: Option<CodexModelInfo>,
    info: Option<CodexInfo>,
    source: Option<Value>,
    cwd: Option<String>,
    model_provider: Option<String>,
    agent_nickname: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexModelInfo {
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexInfo {
    model: Option<String>,
    model_name: Option<String>,
    total_token_usage: Option<CodexTokenUsage>,
    last_token_usage: Option<CodexTokenUsage>,
}

#[derive(Debug, Deserialize, Clone)]
struct CodexTokenUsage {
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    reasoning_output_tokens: Option<i64>,
    total_tokens: Option<i64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CodexTotals {
    input: i64,
    output: i64,
    cached: i64,
    reasoning: i64,
}

impl CodexTotals {
    fn from_usage(usage: &CodexTokenUsage) -> Self {
        let cached = usage
            .cached_input_tokens
            .unwrap_or(0)
            .max(usage.cache_read_input_tokens.unwrap_or(0))
            .max(0);
        Self {
            input: usage.input_tokens.unwrap_or(0).max(0),
            output: usage.output_tokens.unwrap_or(0).max(0),
            cached,
            reasoning: usage.reasoning_output_tokens.unwrap_or(0).max(0),
        }
    }

    fn total(self) -> i64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cached)
            .saturating_add(self.reasoning)
    }

    fn delta_from(self, previous: Self) -> Option<Self> {
        if self.input < previous.input
            || self.output < previous.output
            || self.cached < previous.cached
            || self.reasoning < previous.reasoning
        {
            return None;
        }
        Some(Self {
            input: self.input - previous.input,
            output: self.output - previous.output,
            cached: self.cached - previous.cached,
            reasoning: self.reasoning - previous.reasoning,
        })
    }

    fn saturating_add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cached: self.cached.saturating_add(other.cached),
            reasoning: self.reasoning.saturating_add(other.reasoning),
        }
    }

    fn looks_like_stale_regression(self, previous: Self, last: Self) -> bool {
        let prev_total = previous.total();
        let curr_total = self.total();
        let last_total = last.total();
        if prev_total <= 0 || curr_total <= 0 || last_total <= 0 {
            return false;
        }
        curr_total.saturating_mul(100) >= prev_total.saturating_mul(98)
            || curr_total.saturating_add(last_total.saturating_mul(2)) >= prev_total
    }

    fn into_tokens(self) -> Self {
        let cached = self.cached.min(self.input).max(0);
        Self {
            input: (self.input - cached).max(0),
            output: self.output.max(0),
            cached,
            reasoning: self.reasoning.max(0),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct CodexParseState {
    current_model: Option<String>,
    previous_totals: Option<CodexTotals>,
    session_is_headless: bool,
    session_id_from_meta: Option<String>,
    session_forked_from_id: Option<String>,
    session_provider: Option<String>,
    session_agent: Option<String>,
    session_workspace: Option<String>,
    forked_child_waiting_for_turn_context: bool,
    forked_child_inherited_baseline: Option<CodexTotals>,
    forked_child_inherited_reported_total: Option<i64>,
}

#[derive(Debug, Clone)]
struct ParsedCodexUsage {
    usage: TokenUsage,
    used_fallback_timestamp: bool,
    dedup_key: Option<String>,
}

fn extract_codex_model(payload: &CodexPayload) -> Option<String> {
    payload
        .model_info
        .as_ref()
        .and_then(|mi| mi.slug.clone())
        .filter(|s| !s.is_empty())
        .or_else(|| payload.model.clone().filter(|s| !s.is_empty()))
        .or_else(|| payload.model_name.clone().filter(|s| !s.is_empty()))
        .or_else(|| {
            payload
                .info
                .as_ref()
                .and_then(extract_codex_model_from_info)
        })
}

fn extract_codex_model_from_info(info: &CodexInfo) -> Option<String> {
    info.model
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| info.model_name.clone().filter(|s| !s.is_empty()))
}

fn scan_codex_file(
    path: &Path,
    filter: Option<&ScanFilter>,
    mut global_dedup: Option<&mut HashSet<String>>,
) -> Result<Vec<TokenUsage>> {
    use std::io::BufReader;

    let file =
        std::fs::File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let fallback_ts = file_modified_timestamp(path);
    let reader = BufReader::new(file);
    let parsed = parse_codex_reader(reader, &session_id, fallback_ts);

    let mut usages = Vec::new();
    for parsed_usage in parsed {
        if let Some(f) = filter {
            if !f.include_timestamp(parsed_usage.usage.timestamp) {
                continue;
            }
        }
        if let (Some(dedup), Some(key)) = (global_dedup.as_deref_mut(), parsed_usage.dedup_key) {
            if !dedup.insert(key) {
                continue;
            }
        }
        usages.push(parsed_usage.usage);
    }

    Ok(usages)
}

fn parse_codex_reader<R: std::io::BufRead>(
    reader: R,
    session_id: &str,
    fallback_ts: i64,
) -> Vec<ParsedCodexUsage> {
    let mut usages = Vec::new();
    let mut pending_model_usages: Vec<ParsedCodexUsage> = Vec::new();
    let mut state = CodexParseState::default();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let mut handled = false;

        if let Ok(entry) = serde_json::from_str::<CodexEntry>(trimmed) {
            if let Some(payload) = entry.payload {
                let payload_model = extract_codex_model(&payload);
                let is_token_count = entry.entry_type == "event_msg"
                    && payload.payload_type.as_deref() == Some("token_count");
                let info_model = if is_token_count {
                    payload
                        .info
                        .as_ref()
                        .and_then(extract_codex_model_from_info)
                } else {
                    None
                };
                let event_model = payload_model.clone().or(info_model.clone());

                if state.forked_child_waiting_for_turn_context {
                    if entry.entry_type == "turn_context" {
                        state.forked_child_waiting_for_turn_context = false;
                        state.current_model = payload_model.clone();
                        handled = true;
                    } else {
                        if is_token_count {
                            if let Some(info) = payload.info.as_ref() {
                                remember_forked_child_inherited_baseline(&mut state, info);
                            }
                        }
                        continue;
                    }
                }

                if !pending_model_usages.is_empty()
                    && event_model.is_none()
                    && !is_token_count
                    && entry.entry_type != "session_meta"
                {
                    flush_pending_codex_usages(&mut pending_model_usages, &mut usages, "unknown");
                }

                if entry.entry_type == "session_meta" {
                    if codex_source_is_exec(payload.source.as_ref()) {
                        state.session_is_headless = true;
                    }
                    if let Some(ref id) = payload.id {
                        state.session_id_from_meta = Some(id.clone());
                    }
                    if let Some(ref forked_from_id) = payload.forked_from_id {
                        state.session_forked_from_id = Some(forked_from_id.clone());
                        state.forked_child_waiting_for_turn_context = true;
                        state.forked_child_inherited_baseline = None;
                        state.forked_child_inherited_reported_total = None;
                    }
                    if let Some(ref provider) = payload.model_provider {
                        state.session_provider = Some(provider.clone());
                    }
                    if let Some(ref agent) = payload.agent_nickname {
                        state.session_agent = Some(agent.clone());
                    }
                    if let Some(ref cwd) = payload.cwd {
                        state.session_workspace = Some(cwd.clone());
                    }
                    handled = true;
                }

                if entry.entry_type == "turn_context" {
                    state.current_model = payload_model.clone();
                    if let Some(model) = state.current_model.clone() {
                        flush_pending_codex_usages(&mut pending_model_usages, &mut usages, &model);
                    }
                    handled = true;
                }

                if is_token_count {
                    let info = match payload.info {
                        Some(i) => i,
                        None => continue,
                    };

                    let model = payload_model
                        .or(info_model)
                        .or_else(|| state.current_model.clone());
                    if let Some(ref model) = model {
                        state.current_model = Some(model.clone());
                        flush_pending_codex_usages(&mut pending_model_usages, &mut usages, model);
                    }

                    let total_usage = info.total_token_usage.as_ref().map(CodexTotals::from_usage);
                    let last_usage = info.last_token_usage.as_ref().map(CodexTotals::from_usage);

                    if forked_child_matches_inherited_baseline(
                        &state,
                        info.total_token_usage.as_ref(),
                        total_usage,
                    ) {
                        if let Some(total) = total_usage {
                            state.previous_totals = Some(total);
                        }
                        state.forked_child_inherited_baseline = None;
                        state.forked_child_inherited_reported_total = None;
                        continue;
                    }
                    state.forked_child_inherited_baseline = None;
                    state.forked_child_inherited_reported_total = None;

                    let (tokens, next_totals) =
                        match (total_usage, last_usage, state.previous_totals) {
                            (Some(total), Some(last), Some(previous)) => {
                                if total == previous {
                                    continue;
                                }
                                if total.delta_from(previous).is_none()
                                    && total.looks_like_stale_regression(previous, last)
                                {
                                    continue;
                                }
                                (last.into_tokens(), Some(total))
                            }
                            (Some(total), Some(last), None) => (last.into_tokens(), Some(total)),
                            (Some(total), None, Some(previous)) => {
                                if total == previous {
                                    continue;
                                }
                                if let Some(delta) = total.delta_from(previous) {
                                    (delta.into_tokens(), Some(total))
                                } else {
                                    state.previous_totals = Some(total);
                                    continue;
                                }
                            }
                            (Some(total), None, None) => (total.into_tokens(), Some(total)),
                            (None, Some(last), Some(previous)) => {
                                (last.into_tokens(), Some(previous.saturating_add(last)))
                            }
                            (None, Some(last), None) => (last.into_tokens(), None),
                            (None, None, _) => continue,
                        };

                    if tokens.input == 0
                        && tokens.output == 0
                        && tokens.cached == 0
                        && tokens.reasoning == 0
                    {
                        continue;
                    }

                    state.previous_totals = next_totals;

                    let parsed_timestamp = entry
                        .timestamp
                        .and_then(|ts| DateTime::parse_from_rfc3339(&ts).ok())
                        .map(|dt| dt.timestamp_millis());
                    let timestamp = parsed_timestamp.unwrap_or(fallback_ts);
                    let provider = state
                        .session_provider
                        .clone()
                        .unwrap_or_else(|| "openai".to_string());

                    let usage = TokenUsage {
                        agent: "codex".into(),
                        model_id: model.clone().unwrap_or_else(|| "unknown".to_string()),
                        provider,
                        input_tokens: tokens.input,
                        output_tokens: tokens.output,
                        cache_read_tokens: tokens.cached,
                        cache_write_tokens: 0,
                        reasoning_tokens: tokens.reasoning,
                        timestamp,
                        session_id: state
                            .session_id_from_meta
                            .clone()
                            .unwrap_or_else(|| session_id.to_string()),
                        workspace_key: state.session_workspace.clone(),
                    };
                    let mut parsed_usage = ParsedCodexUsage {
                        usage,
                        used_fallback_timestamp: parsed_timestamp.is_none(),
                        dedup_key: None,
                    };

                    if let Some(model) = model.as_deref() {
                        set_codex_dedup_key(&mut parsed_usage, model);
                        usages.push(parsed_usage);
                    } else {
                        pending_model_usages.push(parsed_usage);
                    }
                    handled = true;
                }
            }

            if handled {
                continue;
            }
        }

        if let Some(mut parsed_usage) =
            parse_codex_headless_line(trimmed, session_id, fallback_ts, &mut state)
        {
            if !pending_model_usages.is_empty() {
                if let Some(model) = state.current_model.clone() {
                    flush_pending_codex_usages(&mut pending_model_usages, &mut usages, &model);
                } else {
                    flush_pending_codex_usages(&mut pending_model_usages, &mut usages, "unknown");
                }
            }
            parsed_usage.usage.workspace_key = state.session_workspace.clone();
            usages.push(parsed_usage);
        }
    }

    flush_pending_codex_usages(&mut pending_model_usages, &mut usages, "unknown");
    usages
}

fn codex_source_is_exec(source: Option<&Value>) -> bool {
    source.and_then(Value::as_str) == Some("exec")
}

fn flush_pending_codex_usages(
    pending: &mut Vec<ParsedCodexUsage>,
    usages: &mut Vec<ParsedCodexUsage>,
    model: &str,
) {
    for mut parsed_usage in pending.drain(..) {
        if !parsed_usage.used_fallback_timestamp {
            set_codex_dedup_key(&mut parsed_usage, model);
        }
        parsed_usage.usage.model_id = model.to_string();
        usages.push(parsed_usage);
    }
}

fn codex_dedup_key(usage: &TokenUsage, model: &str) -> String {
    format!(
        "codex:token_count:{}:{}:{}:{}:{}:{}:{}:{}",
        usage.timestamp,
        usage.provider,
        model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
        usage.reasoning_tokens
    )
}

fn set_codex_dedup_key(parsed_usage: &mut ParsedCodexUsage, model: &str) {
    if parsed_usage.dedup_key.is_none() && !parsed_usage.used_fallback_timestamp {
        parsed_usage.dedup_key = Some(codex_dedup_key(&parsed_usage.usage, model));
    }
}

fn reported_total_tokens(usage: &CodexTokenUsage) -> Option<i64> {
    usage.total_tokens.filter(|total| *total >= 0)
}

fn remember_forked_child_inherited_baseline(state: &mut CodexParseState, info: &CodexInfo) {
    let Some(total_usage) = info.total_token_usage.as_ref() else {
        return;
    };

    let totals = CodexTotals::from_usage(total_usage);
    state.previous_totals = Some(totals);
    state.forked_child_inherited_baseline = Some(totals);
    state.forked_child_inherited_reported_total = reported_total_tokens(total_usage);
}

fn forked_child_matches_inherited_baseline(
    state: &CodexParseState,
    total_usage: Option<&CodexTokenUsage>,
    totals: Option<CodexTotals>,
) -> bool {
    if let (Some(usage), Some(baseline)) =
        (total_usage, state.forked_child_inherited_reported_total)
    {
        if reported_total_tokens(usage) == Some(baseline) {
            return true;
        }
    }

    if let (Some(totals), Some(baseline)) = (totals, state.forked_child_inherited_baseline) {
        return totals == baseline;
    }

    false
}

fn parse_codex_headless_line(
    line: &str,
    session_id: &str,
    fallback_ts: i64,
    state: &mut CodexParseState,
) -> Option<ParsedCodexUsage> {
    let value: Value = serde_json::from_str(line).ok()?;

    if let Some(model) = extract_codex_model_from_value(&value) {
        state.current_model = Some(model);
    }

    let usage = value
        .get("usage")
        .or_else(|| value.get("data").and_then(|data| data.get("usage")))
        .or_else(|| value.get("result").and_then(|data| data.get("usage")))
        .or_else(|| value.get("response").and_then(|data| data.get("usage")))?;

    let input = extract_i64(usage.get("input_tokens"))
        .or_else(|| extract_i64(usage.get("prompt_tokens")))
        .or_else(|| extract_i64(usage.get("input")))
        .unwrap_or(0)
        .max(0);
    let output = extract_i64(usage.get("output_tokens"))
        .or_else(|| extract_i64(usage.get("completion_tokens")))
        .or_else(|| extract_i64(usage.get("output")))
        .unwrap_or(0)
        .max(0);
    let cached = extract_i64(usage.get("cached_input_tokens"))
        .or_else(|| extract_i64(usage.get("cache_read_input_tokens")))
        .or_else(|| extract_i64(usage.get("cached_tokens")))
        .unwrap_or(0)
        .max(0);
    let cached = cached.min(input);
    let model = extract_codex_model_from_value(&value)
        .or_else(|| value.get("data").and_then(extract_codex_model_from_value))
        .or_else(|| state.current_model.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let parsed_timestamp = extract_timestamp_from_value(&value);
    let timestamp = parsed_timestamp.unwrap_or(fallback_ts);

    if input == 0 && output == 0 && cached == 0 {
        return None;
    }

    let provider = state
        .session_provider
        .clone()
        .unwrap_or_else(|| "openai".to_string());
    let usage = TokenUsage {
        agent: "codex".into(),
        model_id: model.clone(),
        provider,
        input_tokens: (input - cached).max(0),
        output_tokens: output,
        cache_read_tokens: cached,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        timestamp,
        session_id: state
            .session_id_from_meta
            .clone()
            .unwrap_or_else(|| session_id.to_string()),
        workspace_key: state.session_workspace.clone(),
    };
    let mut parsed_usage = ParsedCodexUsage {
        usage,
        used_fallback_timestamp: parsed_timestamp.is_none(),
        dedup_key: None,
    };
    set_codex_dedup_key(&mut parsed_usage, &model);
    Some(parsed_usage)
}

fn extract_codex_model_from_value(value: &Value) -> Option<String> {
    extract_string(value.get("model"))
        .or_else(|| extract_string(value.get("model_name")))
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| extract_string(data.get("model")))
        })
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| extract_string(data.get("model_name")))
        })
        .or_else(|| {
            value
                .get("response")
                .and_then(|data| extract_string(data.get("model")))
        })
}

fn extract_timestamp_from_value(value: &Value) -> Option<i64> {
    value
        .get("timestamp")
        .or_else(|| value.get("time"))
        .or_else(|| value.get("created_at"))
        .or_else(|| value.get("data").and_then(|data| data.get("timestamp")))
        .and_then(parse_timestamp_value)
}

fn parse_timestamp_value(value: &Value) -> Option<i64> {
    if let Some(ms) = value.as_i64() {
        return Some(ms);
    }
    let raw = value.as_str()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(ms) = raw.parse::<i64>() {
        return Some(ms);
    }
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn extract_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|n| i64::try_from(n).ok()))
        .or_else(|| value.as_f64().map(|n| n as i64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse::<i64>().ok()))
}

fn extract_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
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

// ============================================================================
// Aggregation
// ============================================================================

/// Aggregate token usage records by day.
pub fn aggregate(records: &[TokenUsage]) -> BTreeMap<String, DailyAggregate> {
    let mut days: BTreeMap<String, DailyAggregate> = BTreeMap::new();
    let pricing = PricingService::load();

    for r in records {
        log::trace!("Aggregating {} session {}", r.agent, r.session_id);

        let date = timestamp_to_date(r.timestamp);
        let day = days.entry(date).or_default();

        let total = r.input_tokens
            + r.output_tokens
            + r.cache_read_tokens
            + r.cache_write_tokens
            + r.reasoning_tokens;
        let cost = pricing.calculate_cost(
            &r.model_id,
            &r.provider,
            CostTokens {
                input: r.input_tokens,
                output: r.output_tokens,
                cache_read: r.cache_read_tokens,
                cache_write: r.cache_write_tokens,
                reasoning: r.reasoning_tokens,
            },
        );
        day.total_tokens += total;
        day.total_cost += cost;
        day.input_tokens += r.input_tokens;
        day.output_tokens += r.output_tokens;
        day.cache_read_tokens += r.cache_read_tokens;
        day.cache_write_tokens += r.cache_write_tokens;
        day.reasoning_tokens += r.reasoning_tokens;

        // Compound key: model|agent so each (model, source) combination is tracked separately
        let model_key = format!("{}|{}", r.model_id, r.agent);
        let model = day.models.entry(model_key).or_default();
        model.tokens += total;
        model.input_tokens += r.input_tokens;
        model.output_tokens += r.output_tokens;
        model.cache_read_tokens += r.cache_read_tokens;
        model.cache_write_tokens += r.cache_write_tokens;
        model.total_cost += cost;
        model.provider = r.provider.clone();
        model.source = r.agent.clone();

        let client = day.clients.entry(r.agent.clone()).or_default();
        client.tokens += total;
        client.cost += cost;
    }

    days
}

fn timestamp_to_date(ts_ms: i64) -> String {
    let secs = ts_ms / 1000;
    let nsecs = ((ts_ms % 1000) * 1_000_000) as u32;
    if let Some(dt) = Utc.timestamp_opt(secs, nsecs).single() {
        dt.format("%Y-%m-%d").to_string()
    } else {
        let days = ts_ms / (24 * 60 * 60 * 1000);
        let ts = days * (24 * 60 * 60 * 1000);
        let fallback_secs = ts / 1000;
        let fallback_nsecs = ((ts % 1000) * 1_000_000) as u32;
        if let Some(dt) = Utc.timestamp_opt(fallback_secs, fallback_nsecs).single() {
            dt.format("%Y-%m-%d").to_string()
        } else {
            "unknown".to_string()
        }
    }
}

/// Strip date suffixes from model names.
/// e.g. "claude-haiku-4-5-20251001" → "claude-haiku-4-5"
/// Returns None if no normalization is needed.
fn normalize_model_name(model: &str) -> Option<String> {
    // Match trailing -YYYYMMDD pattern (8 digits after last hyphen)
    let bytes = model.as_bytes();
    if bytes.len() < 9 {
        return None;
    }
    // Check if last 9 chars match "-YYYYMMDD"
    let suffix = &bytes[bytes.len() - 9..];
    if suffix[0] == b'-' && suffix[1..].iter().all(|b| b.is_ascii_digit()) {
        return Some(model[..bytes.len() - 9].to_string());
    }
    None
}

/// Capitalize the first letter of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Normalize provider name to proper casing.
fn normalize_provider(provider: &str) -> String {
    match provider.to_lowercase().as_str() {
        "openai" => "OpenAI".into(),
        "deepseek" => "DeepSeek".into(),
        "anthropic" => "Anthropic".into(),
        "google" => "Google".into(),
        "meta" => "Meta".into(),
        "alibaba" => "Alibaba".into(),
        "moonshot" => "Moonshot".into(),
        "github" => "GitHub".into(),
        "nous" => "Nous".into(),
        "" | "unknown" => "Unknown".into(),
        other => capitalize_first(other),
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_scan_claude_code_detects_usage() {
        let temp = TempDir::new().unwrap();
        let projects_dir = temp
            .path()
            .join(".claude")
            .join("projects")
            .join("-Users-test-project");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let session_file = projects_dir.join("test-session.jsonl");
        let mut file = std::fs::File::create(&session_file).unwrap();
        writeln!(
            file,
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00.000Z","requestId":"req-001","message":{{"id":"msg-001","model":"claude-opus-4-6","usage":{{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":20,"cache_creation_input_tokens":5}}}}}}"#
        )
        .unwrap();

        // Simulate scanning the claude project dir
        let client_def = crate::clients::find_client("claude").unwrap();
        let usages = scan_file(&session_file, client_def, None, None).unwrap();
        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].model_id, "claude-opus-4-6");
        assert_eq!(usages[0].input_tokens, 100);
        assert_eq!(usages[0].output_tokens, 50);
        assert_eq!(usages[0].cache_read_tokens, 20);
        assert_eq!(usages[0].cache_write_tokens, 5);
        assert_eq!(usages[0].provider, "Anthropic");
    }

    #[test]
    fn test_scan_codex_detects_usage() {
        let temp = TempDir::new().unwrap();
        let session_file = temp.path().join("rollout-test.jsonl");
        let mut file = std::fs::File::create(&session_file).unwrap();
        writeln!(
            file,
            r#"{{"type":"session_meta","payload":{{"cwd":"/Users/test/project","model_provider":"openai"}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"type":"turn_context","payload":{{"model_info":{{"slug":"gpt-5.5"}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":200,"output_tokens":50,"cached_input_tokens":40}},"last_token_usage":{{"input_tokens":200,"output_tokens":50,"cached_input_tokens":40}}}}}}}}"#
        )
        .unwrap();

        let usages = scan_codex_file(&session_file, None, None).unwrap();
        assert!(!usages.is_empty(), "Should detect Codex usage");
        assert_eq!(usages[0].model_id, "gpt-5.5");
        assert_eq!(usages[0].provider, "openai");
        assert_eq!(usages[0].input_tokens, 160);
        assert_eq!(usages[0].output_tokens, 50);
        assert_eq!(usages[0].cache_read_tokens, 40);
        assert_eq!(
            usages[0].workspace_key.as_deref(),
            Some("/Users/test/project")
        );
    }

    #[test]
    fn test_discover_codex_files_includes_archived_sessions() {
        let temp = TempDir::new().unwrap();
        let codex_home = temp.path().join(".codex");
        let sessions_dir = codex_home.join("sessions");
        let archived_dir = codex_home.join("archived_sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        std::fs::create_dir_all(&archived_dir).unwrap();
        std::fs::write(sessions_dir.join("active.jsonl"), "{}\n").unwrap();
        std::fs::write(archived_dir.join("archived.jsonl"), "{}\n").unwrap();

        let files = discover_codex_files(&sessions_dir, "*.jsonl");

        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("active.jsonl")));
        assert!(files.iter().any(|path| path.ends_with("archived.jsonl")));
    }

    #[test]
    fn test_scan_codex_global_dedup_filters_duplicate_snapshots() {
        let temp = TempDir::new().unwrap();
        let first_file = temp.path().join("first.jsonl");
        let second_file = temp.path().join("second.jsonl");
        let content = concat!(
            r#"{"type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            "\n",
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":200,"output_tokens":50,"cached_input_tokens":40},"last_token_usage":{"input_tokens":200,"output_tokens":50,"cached_input_tokens":40}}}}"#,
            "\n"
        );
        std::fs::write(&first_file, content).unwrap();
        std::fs::write(&second_file, content).unwrap();
        let mut dedup = HashSet::new();

        let first = scan_codex_file(&first_file, None, Some(&mut dedup)).unwrap();
        let second = scan_codex_file(&second_file, None, Some(&mut dedup)).unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(second.len(), 0);
    }

    #[test]
    fn test_scan_codex_assigns_late_turn_context_model() {
        let temp = TempDir::new().unwrap();
        let session_file = temp.path().join("late-model.jsonl");
        let content = concat!(
            r#"{"timestamp":"2026-04-27T10:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1},"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":3,"reasoning_output_tokens":1}}}}"#,
            "\n",
            r#"{"timestamp":"2026-04-27T10:00:04Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#,
            "\n"
        );
        std::fs::write(&session_file, content).unwrap();

        let usages = scan_codex_file(&session_file, None, None).unwrap();

        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].model_id, "gpt-5.5");
        assert_eq!(usages[0].input_tokens, 8);
        assert_eq!(usages[0].cache_read_tokens, 2);
        assert_eq!(usages[0].reasoning_tokens, 1);
    }

    #[test]
    fn test_scan_codex_skips_forked_child_inherited_baseline() {
        let temp = TempDir::new().unwrap();
        let session_file = temp.path().join("forked.jsonl");
        let content = concat!(
            r#"{"type":"session_meta","payload":{"id":"child","forked_from_id":"parent"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-05T21:51:57.994Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
            "\n",
            r#"{"timestamp":"2026-05-05T21:51:58.947Z","type":"turn_context","payload":{"model":"gpt-5.5","cwd":"/repo-child"}}"#,
            "\n",
            r#"{"timestamp":"2026-05-05T21:51:58.948Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":116000,"cached_input_tokens":114000,"output_tokens":1000,"total_tokens":117000},"last_token_usage":{"input_tokens":73000,"cached_input_tokens":72000,"output_tokens":500,"total_tokens":73500}}}}"#,
            "\n",
            r#"{"timestamp":"2026-05-05T21:51:59.253Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":117500,"cached_input_tokens":115000,"output_tokens":1200,"reasoning_output_tokens":50,"total_tokens":118700},"last_token_usage":{"input_tokens":1500,"cached_input_tokens":1000,"output_tokens":200,"reasoning_output_tokens":50,"total_tokens":1700}}}}"#,
            "\n"
        );
        std::fs::write(&session_file, content).unwrap();

        let usages = scan_codex_file(&session_file, None, None).unwrap();

        assert_eq!(usages.len(), 1);
        assert_eq!(usages[0].input_tokens, 500);
        assert_eq!(usages[0].output_tokens, 200);
        assert_eq!(usages[0].cache_read_tokens, 1000);
        assert_eq!(usages[0].reasoning_tokens, 50);
    }

    #[test]
    fn test_match_pattern() {
        assert!(match_pattern("session.jsonl", "*.jsonl"));
        assert!(match_pattern("data.json", "*.json|*.jsonl"));
        assert!(!match_pattern("data.txt", "*.json|*.jsonl"));
        assert!(match_pattern("usage.csv", "usage*.csv"));
        assert!(match_pattern("usage.work.csv", "usage*.csv"));
        assert!(match_pattern("T-2026-01-01.json", "T-*.json"));
        assert!(!match_pattern("other.json", "T-*.json"));
        assert!(match_pattern("ui_messages.json", "ui_messages.json"));
        assert!(match_pattern("state.db", "state.db"));
        // backup exclusion is handled at the per-client level, not in the generic matcher
    }
}
