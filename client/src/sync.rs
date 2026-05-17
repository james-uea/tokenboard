//! Submits scanned token data to the tokenboard leaderboard API.

use anyhow::{bail, Context, Result};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::scanner::{DailyAggregate, ScanFilter, TokenUsage};

/// Configuration for the sync client.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// tokenboard API base URL (e.g. "http://localhost:3000")
    pub api_url: String,
    /// User-bound Tokenboard API token for authentication
    pub api_token: String,
    /// Display name shown in leaderboard UI
    pub display_name: String,
    /// GitHub username to submit as
    pub github_username: String,
}

/// The submission payload sent to POST /api/submit
#[derive(Debug, Serialize)]
pub struct SubmitPayload {
    pub username: String,
    pub display_name: String,
    pub contributions: Vec<ContributionPayload>,
}

/// Flat contribution format matching the server's expected schema.
#[derive(Debug, Serialize)]
pub struct ContributionPayload {
    pub date: String,
    pub total_tokens: i64,
    pub total_cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    /// Map of model_id -> { tokens, input, output }
    pub models: HashMap<String, ModelEntry>,
    /// Map of client_id -> { tokens, cost }
    pub clients: HashMap<String, ClientEntry>,
}

#[derive(Debug, Serialize)]
pub struct ModelEntry {
    pub tokens: i64,
    pub input: i64,
    pub output: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub cost: f64,
    pub provider: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ClientEntry {
    pub tokens: i64,
    pub cost: f64,
}

/// Response from the tokenboard API.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SubmitResponse {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub contributions_updated: usize,
}

/// Build a flat submit payload from aggregated daily data.
pub fn build_payload(
    username: &str,
    display_name: &str,
    days: &BTreeMap<String, DailyAggregate>,
) -> SubmitPayload {
    let contributions: Vec<ContributionPayload> = days
        .iter()
        .filter(|(_, day)| day.total_tokens > 0)
        .map(|(date, day)| {
            let models: HashMap<String, ModelEntry> = day
                .models
                .iter()
                .map(|(name, m)| {
                    (
                        name.clone(),
                        ModelEntry {
                            tokens: m.tokens,
                            input: m.input_tokens,
                            output: m.output_tokens,
                            cache_read: m.cache_read_tokens,
                            cache_write: m.cache_write_tokens,
                            cost: m.total_cost,
                            provider: m.provider.clone(),
                            source: m.source.clone(),
                        },
                    )
                })
                .collect();

            let clients: HashMap<String, ClientEntry> = day
                .clients
                .iter()
                .map(|(name, c)| {
                    (
                        name.clone(),
                        ClientEntry {
                            tokens: c.tokens,
                            cost: c.cost,
                        },
                    )
                })
                .collect();

            ContributionPayload {
                date: date.clone(),
                total_tokens: day.total_tokens,
                total_cost: day.total_cost,
                input_tokens: day.input_tokens,
                output_tokens: day.output_tokens,
                cache_read_tokens: day.cache_read_tokens,
                cache_write_tokens: day.cache_write_tokens,
                reasoning_tokens: day.reasoning_tokens,
                models,
                clients,
            }
        })
        .collect();

    SubmitPayload {
        username: username.to_string(),
        display_name: display_name.to_string(),
        contributions,
    }
}

/// Submit the payload to the tokenboard API.
pub fn submit(config: &SyncConfig, payload: &SubmitPayload) -> Result<SubmitResponse> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/api/submit", config.api_url.trim_end_matches('/'));

    info!("Submitting to {} as {}", url, config.github_username);
    debug!(
        "Payload: {} contributions, {} total tokens",
        payload.contributions.len(),
        payload
            .contributions
            .iter()
            .map(|c| c.total_tokens)
            .sum::<i64>()
    );

    let response = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", &config.api_token))
        .json(payload)
        .send()
        .context("Failed to send submission request")?;

    let status = response.status();

    if !status.is_success() {
        let body = response
            .text()
            .unwrap_or_else(|_| "unknown error".to_string());
        bail!(
            "Server returned {} {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            body
        );
    }

    let result: SubmitResponse = response.json().context("Failed to parse server response")?;

    Ok(result)
}

/// Get the user's rank from the leaderboard.
pub fn get_rank(config: &SyncConfig, username: &str) -> Result<Option<i64>> {
    let client = reqwest::blocking::Client::new();
    let url = format!(
        "{}/api/leaderboard?username={}",
        config.api_url.trim_end_matches('/'),
        urlencoding(username)
    );

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", &config.api_token))
        .send()
        .context("Failed to fetch leaderboard")?;

    if !response.status().is_success() {
        return Ok(None);
    }

    let leaderboard_response: serde_json::Value =
        response.json().context("Failed to parse leaderboard")?;
    let leaderboard = leaderboard_response
        .get("leaderboard")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let rank = leaderboard
        .iter()
        .position(|entry| {
            entry
                .get("username")
                .and_then(|v| v.as_str())
                .map(|u| u == username)
                .unwrap_or(false)
        })
        .map(|pos| pos as i64 + 1);

    Ok(rank)
}

fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// Scan local files and optionally sync to the leaderboard.
///
/// When `dry_run` is `true`, the scan runs but no data is submitted.
/// `filter` can restrict scanning by agent and/or date range.
pub fn scan_and_sync(
    config: &SyncConfig,
    dry_run: bool,
    filter: Option<&ScanFilter>,
) -> Result<()> {
    let records = crate::scanner::scan_all(filter).context("Failed to scan for session data")?;

    if records.is_empty() {
        info!("No token usage data found. Have you used any AI coding agents?");
        return Ok(());
    }

    info!("Found {} token usage records", records.len());

    let days = crate::scanner::aggregate(&records);
    info!("Aggregated into {} days of activity", days.len());

    let total_tokens: i64 = days.values().map(|d| d.total_tokens).sum();

    if dry_run {
        print_dry_run(&days, total_tokens, &records);
        return Ok(());
    }

    let payload = build_payload(&config.github_username, &config.display_name, &days);

    let result = submit(config, &payload)?;

    let rank_username = if result.username.is_empty() {
        config.github_username.as_str()
    } else {
        result.username.as_str()
    };
    let rank = get_rank(config, rank_username).unwrap_or(None);

    info!(
        "Sync complete! Updated {} days, {} total tokens. Rank: {}",
        result.contributions_updated,
        total_tokens,
        rank.map_or("N/A".to_string(), |r| format!("#{}", r))
    );

    Ok(())
}

/// Print a human-readable summary of what would be submitted.
fn print_dry_run(
    days: &BTreeMap<String, DailyAggregate>,
    total_tokens: i64,
    records: &[TokenUsage],
) {
    println!();
    println!("{:=^50}", " DRY RUN — nothing submitted ");
    println!();
    println!("  days:        {}", days.len());
    println!("  records:     {}", records.len());
    println!("  total tokens: {}", total_tokens);
    println!();

    for (date, day) in days {
        println!("  {}", date);
        println!("    tokens:    {:>12}", day.total_tokens);
        println!("    cost:      ${:>11.4}", day.total_cost);
        println!("    models:    {:>12}", day.models.len());
        println!(
            "    clients:   {:>12}  ({})",
            day.clients.len(),
            day.clients
                .keys()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!();
    }

    println!("{:=^50}", "");
    println!();
}
