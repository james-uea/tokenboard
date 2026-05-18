//! tokenboard — desktop client for syncing AI coding agent token usage to the leaderboard.
//!
//! Usage:
//!   tokenboard         Show help
//!   tokenboard setup   Sign in with GitHub and configure Tokenboard
//!   tokenboard scan    Scan and print local token usage (dry run, no submission)
//!   tokenboard sync    Scan local session data and submit to the leaderboard
//!   tokenboard autosync install  Schedule sync every 3 hours
//!
//! Configuration is stored in ~/.tokenboard/config.toml and can also be set via
//! environment variables or .env files:
//!   TOKENBOARD_API_URL   — API base URL (default: https://tokenboard.net)
//!   TOKENBOARD_API_TOKEN — User-bound Tokenboard API token for authentication
//!   TOKENBOARD_API_KEY   — Legacy shared API key fallback
//!   TOKENBOARD_GITHUB_USERNAME  — Your GitHub username on the leaderboard
//!   TOKENBOARD_DISPLAY_NAME  — Display name shown on the leaderboard
//!   TOKENBOARD_AUTO_UPDATE — Enable/disable release auto-update (default: true)
//!   TOKENBOARD_UPDATE_REPO — GitHub repo for releases (default: james-uea/tokenboard)
//!   TOKENBOARD_UPDATE_GITHUB_TOKEN — Optional token for authenticated GitHub release access

mod clients;
mod parsers;
mod pricing;
mod scanner;
mod scheduler;
mod sync;
mod updater;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(
    name = "tokenboard",
    version,
    about = "Sync AI coding agent token usage to the leaderboard.",
    after_help = "Run `tokenboard setup` first if you haven't configured yet.\n\
                  \n\
                  Supported agents: 21 AI coding agents — see `tokenboard scan --help`"
)]
struct Cli {
    /// Increase log verbosity (-v = debug, default = info).
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Date-range filtering flags shared by the `scan` and `sync` subcommands.
///
/// `--today` and `--week` are shortcuts that conflict with `--since`/`--until`.
#[derive(Debug, clap::Args)]
#[group(required = false, multiple = false)]
struct DateFilter {
    /// Only include today's usage.
    #[arg(long, conflicts_with_all = &["since", "until", "week"])]
    today: bool,

    /// Only include the last 7 days.
    #[arg(long, conflicts_with_all = &["since", "until", "today"])]
    week: bool,

    /// Start date (inclusive, YYYY-MM-DD).
    #[arg(long, conflicts_with = "today", requires = "until")]
    since: Option<String>,

    /// End date (inclusive, YYYY-MM-DD).
    #[arg(long, conflicts_with = "today", requires = "since")]
    until: Option<String>,
}

/// Agent / client filtering flags shared by `scan` and `sync`.
#[derive(Debug, clap::Args)]
struct ClientFilter {
    /// Filter by agent. Repeatable or comma-separated (e.g. -c claude,codex).
    #[arg(
        short = 'c',
        long = "client",
        value_delimiter = ',',
        action = clap::ArgAction::Append,
        help = "Filter by agent. Repeatable or comma-separated (e.g. -c claude,codex). 21 agents supported: opencode, claude, codex, cursor, gemini, amp, droid, openclaw, pi, kimi, qwen, roocode, kilocode, mux, kilo, crush, hermes, copilot, goose, codebuff, antigravity."
    )]
    clients: Vec<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scan and print local token usage (dry run, no submission).
    Scan {
        #[command(flatten)]
        date: DateFilter,

        #[command(flatten)]
        client: ClientFilter,
    },

    /// Scan local session data and submit to the leaderboard.
    Sync {
        /// Show what would be submitted without actually submitting.
        #[arg(short = 'n', long)]
        dry_run: bool,

        /// Skip the auto-update check for this sync run.
        #[arg(long)]
        no_update: bool,

        /// Fail instead of launching the setup wizard when config is missing.
        #[arg(long, hide = true)]
        no_setup: bool,

        #[command(flatten)]
        date: DateFilter,

        #[command(flatten)]
        client: ClientFilter,
    },

    /// Sign in with GitHub and configure Tokenboard.
    #[command(
        after_help = "Defaults to https://tokenboard.net. You can choose a custom self-hosted server during setup.\nSetup opens GitHub auth in your browser and can enable autosync at the end."
    )]
    Setup,

    /// Check for and install tokenboard CLI updates.
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },

    /// Manage automatic background sync every 3 hours.
    #[command(name = "autosync", visible_alias = "auto-sync")]
    Autosync {
        #[command(subcommand)]
        command: AutosyncCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AutosyncCommand {
    /// Install the OS-native 3-hour sync schedule.
    Install,
    /// Remove the automatic sync schedule.
    Uninstall,
    /// Show whether automatic sync is installed and active.
    Status,
}

#[derive(Debug, Subcommand)]
enum UpdateCommand {
    /// Check GitHub Releases for a newer tokenboard binary.
    Check,
    /// Download and install the latest newer tokenboard binary.
    Install,
}

// ============================================================================
// Persistent configuration (unchanged)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default = "default_api_url")]
    api_url: String,
    #[serde(default, alias = "api_key")]
    api_token: String,
    #[serde(default, alias = "username")]
    github_username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default = "default_auto_update")]
    auto_update: bool,
    #[serde(default = "default_update_repo")]
    update_repo: String,
    #[serde(default)]
    update_github_token: String,
}

fn default_api_url() -> String {
    "https://tokenboard.net".to_string()
}

fn default_auto_update() -> bool {
    true
}

fn default_update_repo() -> String {
    updater::default_repo().to_string()
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self {
            api_url: default_api_url(),
            api_token: String::new(),
            github_username: String::new(),
            display_name: String::new(),
            auto_update: default_auto_update(),
            update_repo: default_update_repo(),
            update_github_token: String::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CliLoginStart {
    code: String,
    login_url: String,
    #[serde(default = "default_login_expires_in")]
    expires_in: u64,
    #[serde(default = "default_login_poll_interval")]
    poll_interval: u64,
}

#[derive(Debug, Deserialize)]
struct CliLoginPoll {
    status: String,
    token: Option<String>,
    user: Option<CliLoginUser>,
}

#[derive(Debug, Deserialize)]
struct CliLoginUser {
    username: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    github_login: String,
}

struct CliLoginResult {
    token: String,
    user: CliLoginUser,
}

fn default_login_expires_in() -> u64 {
    600
}

fn default_login_poll_interval() -> u64 {
    2
}

// ============================================================================
// Entry point
// ============================================================================

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Log level: default = info, -v = debug
    let log_level = match cli.verbose {
        0 => "info",
        _ => "debug",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .format_timestamp(None)
        .init();

    match cli.command {
        Some(Command::Scan { date, client }) => cmd_scan(date, client)?,
        Some(Command::Sync {
            dry_run,
            no_update,
            no_setup,
            date,
            client,
        }) => cmd_sync(dry_run, no_update, !no_setup, date, client)?,
        Some(Command::Setup) => {
            if let Err(error) = cmd_setup() {
                let message = error.to_string();
                if message == setup_cancelled_message() {
                    eprintln!("{message}");
                    std::process::exit(1);
                }
                return Err(error);
            }
        }
        Some(Command::Update { command }) => cmd_update(command)?,
        Some(Command::Autosync { command }) => cmd_autosync(command)?,
        None => {
            // No subcommand → print friendly help and exit
            Cli::command().print_help()?;
            println!();
        }
    }

    Ok(())
}

// ============================================================================
// Date-filter helpers
// ============================================================================

/// Resolve `DateFilter` into an optional `(since_ms, until_ms)` pair (both
/// Unix-milliseconds, inclusive).  Returns `None` when no date filtering
/// was requested so the scanner can unconditionally include everything.
fn resolve_date_range(filter: &DateFilter) -> Result<Option<(i64, i64)>> {
    use chrono::{NaiveDate, TimeZone, Utc};

    if let (Some(since_raw), Some(until_raw)) = (&filter.since, &filter.until) {
        let parse = |raw: &str| -> Result<i64> {
            let d = NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .with_context(|| format!("Invalid date '{}' — expected YYYY-MM-DD", raw))?;
            Ok(Utc
                .from_utc_datetime(&d.and_hms_opt(0, 0, 0).unwrap())
                .timestamp_millis())
        };
        let since = parse(since_raw)?;
        // End of day (inclusive)
        let until = parse(until_raw)? + 24 * 60 * 60 * 1000 - 1;
        anyhow::ensure!(since <= until, "--since must be before or equal to --until");
        return Ok(Some((since, until)));
    }

    if filter.today {
        let today = Utc::now().date_naive();
        let since = Utc
            .from_utc_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
            .timestamp_millis();
        let until = since + 24 * 60 * 60 * 1000 - 1;
        return Ok(Some((since, until)));
    }

    if filter.week {
        let end = Utc::now().date_naive();
        let start = end - chrono::Duration::days(6);
        let since = Utc
            .from_utc_datetime(&start.and_hms_opt(0, 0, 0).unwrap())
            .timestamp_millis();
        let until = Utc
            .from_utc_datetime(&end.and_hms_opt(0, 0, 0).unwrap())
            .timestamp_millis()
            + 24 * 60 * 60 * 1000
            - 1;
        return Ok(Some((since, until)));
    }

    Ok(None) // no date filter
}

// ============================================================================
// Client-filter helpers
// ============================================================================

/// Validate and normalise `--client` values.
///
/// Unknown IDs produce a warning but are **not** rejected so the tool remains
/// forward-compatible with new agents.
fn resolve_client_filter(raw: &ClientFilter) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for c in &raw.clients {
        let lower = c.trim().to_lowercase();
        if lower.is_empty() {
            continue;
        }
        if clients::find_client(&lower).is_none() {
            eprintln!("⚠  Unknown agent '{}' — will try to scan anyway", lower);
        }
        if seen.insert(lower.clone()) {
            out.push(lower);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

// ============================================================================
// Commands
// ============================================================================

fn cmd_scan(date: DateFilter, client: ClientFilter) -> Result<()> {
    let clients = resolve_client_filter(&client);
    let date_range = resolve_date_range(&date)?;

    let filter_desc = describe_filters(clients.as_ref(), date_range);
    println!(
        "🔍 Scanning for AI coding agent session data{}...\n",
        filter_desc
    );

    let filter = scanner::ScanFilter {
        clients: clients.clone(),
        since_ms: date_range.map(|(s, _)| s),
        until_ms: date_range.map(|(_, u)| u),
    };

    let records = scanner::scan_all(Some(&filter)).context("Failed to scan for session data")?;

    if records.is_empty() {
        print_no_data_found(&clients, &date_range);
        return Ok(());
    }

    print_scan_summary(&records);

    Ok(())
}

fn cmd_sync(
    dry_run: bool,
    no_update: bool,
    allow_setup: bool,
    date: DateFilter,
    client: ClientFilter,
) -> Result<()> {
    let config = load_config_with_setup(allow_setup)?;
    let update_config = load_update_config();

    if !dry_run && !no_update && update_config.auto_update {
        run_auto_update(&update_config);
    }

    let clients = resolve_client_filter(&client);
    let date_range = resolve_date_range(&date)?;

    let filter_desc = describe_filters(clients.as_ref(), date_range);
    println!(
        "🔍 Scanning for AI coding agent session data{}...\n",
        filter_desc
    );

    let filter = scanner::ScanFilter {
        clients: clients.clone(),
        since_ms: date_range.map(|(s, _)| s),
        until_ms: date_range.map(|(_, u)| u),
    };

    sync::scan_and_sync(&config, dry_run, Some(&filter))?;

    Ok(())
}

fn cmd_update(command: UpdateCommand) -> Result<()> {
    let config = load_update_config();
    match command {
        UpdateCommand::Check => match updater::check(&config)? {
            updater::CheckOutcome::UpToDate { current, latest } => {
                println!("tokenboard is up to date ({current}); latest release is {latest}.");
            }
            updater::CheckOutcome::UpdateAvailable {
                current,
                latest,
                asset_name,
            } => {
                println!(
                    "tokenboard {latest} is available (current: {current}, asset: {asset_name})."
                );
            }
        },
        UpdateCommand::Install => match updater::install_latest(&config)? {
            updater::InstallOutcome::AlreadyCurrent { current, latest } => {
                println!("tokenboard is up to date ({current}); latest release is {latest}.");
            }
            updater::InstallOutcome::Installed { version, backup } => {
                println!(
                    "Updated tokenboard to {version}. Previous binary saved at {}.",
                    backup.display()
                );
            }
            updater::InstallOutcome::StagedForWindows { version, helper } => {
                println!(
                    "tokenboard {version} is staged. Windows will apply it after this process exits via {}.",
                    helper.display()
                );
            }
        },
    }
    Ok(())
}

fn run_auto_update(config: &updater::UpdateConfig) {
    match updater::install_latest(config) {
        Ok(updater::InstallOutcome::AlreadyCurrent { .. }) => {}
        Ok(updater::InstallOutcome::Installed { version, .. }) => {
            eprintln!("Updated tokenboard to {version}; the new version will run next time.");
        }
        Ok(updater::InstallOutcome::StagedForWindows { version, .. }) => {
            eprintln!("Staged tokenboard {version}; Windows will apply it after this sync exits.");
        }
        Err(error) => {
            eprintln!("⚠  Auto-update skipped: {error}");
        }
    }
}

// ============================================================================
// Setup
// ============================================================================

fn cmd_setup() -> Result<()> {
    let existing = load_config_file();
    let cfg = run_setup(&existing)?;
    save_config_file(&cfg)?;
    println!("Configuration complete. You can now run:");
    println!("  tokenboard scan     (dry-run — see your usage)");
    println!("  tokenboard sync     (submit to the leaderboard)");

    let mut stdout = io::stdout();
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    if prompt_yes_no(
        &mut stdin,
        &mut stdout,
        "👉 Install automatic sync every 3 hours? [Y/n]: ",
        true,
    ) {
        match scheduler::install() {
            Ok(message) => {
                println!("{message}");
                println!("Automatic sync is enabled. Check it with `tokenboard autosync status`.");
            }
            Err(error) => {
                eprintln!("⚠  Failed to install automatic sync: {error}");
                eprintln!("Run `tokenboard autosync install` after setup to try again.");
            }
        }
    } else {
        println!("Enable it later with `tokenboard autosync install`.");
    }
    Ok(())
}

fn cmd_autosync(command: AutosyncCommand) -> Result<()> {
    match command {
        AutosyncCommand::Install => {
            ensure_persistent_config_for_autosync()?;
            let message = scheduler::install()?;
            println!("{message}");
        }
        AutosyncCommand::Uninstall => {
            let message = scheduler::uninstall()?;
            println!("{message}");
        }
        AutosyncCommand::Status => {
            let message = scheduler::status()?;
            println!("{message}");
        }
    }
    Ok(())
}

fn run_setup(existing: &ConfigFile) -> Result<ConfigFile> {
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let mut stdout = io::stdout();
    run_setup_with_io(existing, &mut stdin, &mut stdout, run_github_login)
}

fn run_setup_with_io<R, W, F>(
    existing: &ConfigFile,
    stdin: &mut R,
    stdout: &mut W,
    login_fn: F,
) -> Result<ConfigFile>
where
    R: BufRead,
    W: Write,
    F: Fn(&str) -> Result<CliLoginResult>,
{
    let mut cfg = existing.clone();

    writeln!(stdout)?;
    writeln!(stdout, "╔══════════════════════════════════════╗")?;
    writeln!(stdout, "║   tokenboard — First-time Setup      ║")?;
    writeln!(stdout, "╚══════════════════════════════════════╝")?;
    writeln!(stdout)?;

    cfg.api_url = prompt_api_url(stdin, stdout, &cfg.api_url)?;

    let missing_required = cfg.github_username.trim().is_empty() || cfg.api_token.trim().is_empty();
    let login_prompt = if missing_required {
        "👉 Sign in with GitHub to configure Tokenboard? [Y/n]: "
    } else {
        "👉 Sign in with GitHub to refresh your Tokenboard session? [y/N]: "
    };

    if prompt_yes_no(stdin, stdout, login_prompt, missing_required) {
        match login_fn(&cfg.api_url) {
            Ok(login) => {
                cfg.api_token = login.token;
                cfg.github_username = if login.user.github_login.trim().is_empty() {
                    login.user.username
                } else {
                    login.user.github_login
                };
                if !login.user.display_name.trim().is_empty() {
                    cfg.display_name = login.user.display_name;
                }
                writeln!(stdout, "Signed in as @{}.", cfg.github_username)?;
            }
            Err(error) => {
                anyhow::bail!("{}", setup_login_failure_message(&cfg.api_url, &error));
            }
        }
    } else if missing_required {
        anyhow::bail!("{}", setup_cancelled_message());
    }

    if cfg.github_username.trim().is_empty() || cfg.api_token.trim().is_empty() {
        anyhow::bail!("{}", setup_cancelled_message());
    }

    // Display name (optional, defaults to GitHub username)
    let default_display_name = if cfg.display_name.trim().is_empty() {
        cfg.github_username.clone()
    } else {
        cfg.display_name.clone()
    };
    write!(stdout, "👉 Display name [{}]: ", default_display_name)?;
    stdout.flush().ok();
    let mut line = String::new();
    stdin.read_line(&mut line).ok();
    let input = line.trim().to_string();
    cfg.display_name = if input.is_empty() {
        default_display_name
    } else {
        input
    };

    writeln!(stdout)?;
    Ok(cfg)
}

fn prompt_api_url<R: BufRead, W: Write>(
    stdin: &mut R,
    stdout: &mut W,
    current_url: &str,
) -> Result<String> {
    let default_url = default_api_url();
    let current = normalize_api_url(current_url);
    let keep_hosted_default = current == default_url;

    if keep_hosted_default {
        if prompt_yes_no(
            stdin,
            stdout,
            "👉 Use https://tokenboard.net as your Tokenboard server? [Y/n]: ",
            true,
        ) {
            return Ok(default_url);
        }
    } else if prompt_yes_no(
        stdin,
        stdout,
        &format!("👉 Keep Tokenboard server {current}? [Y/n]: "),
        true,
    ) {
        return Ok(current);
    }

    loop {
        let input = prompt_line(stdin, stdout, "👉 Tokenboard server URL: ");
        if !input.trim().is_empty() {
            return Ok(normalize_api_url(&input));
        }
        writeln!(stdout, "⚠  Tokenboard server URL cannot be empty.")?;
    }
}

fn setup_login_failure_message(api_url: &str, error: &anyhow::Error) -> String {
    let detail = sanitize_error_message(error);
    format!(
        "GitHub sign-in failed for {api_url}: {detail}\nRetry setup, or check that this Tokenboard server is reachable and configured for GitHub auth."
    )
}

fn setup_cancelled_message() -> &'static str {
    "Setup cancelled. Run `tokenboard setup` when you're ready to sign in with GitHub."
}

fn sanitize_error_message(error: &anyhow::Error) -> String {
    let message = error.to_string();
    let lower = message.to_lowercase();
    if lower.contains("<html")
        || lower.contains("<!doctype")
        || lower.contains("<body")
        || lower.contains("<pre")
    {
        return "Tokenboard server returned an unreadable error response.".to_string();
    }
    message
}

fn prompt_line<R: BufRead, W: Write>(stdin: &mut R, stdout: &mut W, prompt: &str) -> String {
    write!(stdout, "{prompt}").ok();
    stdout.flush().ok();
    let mut line = String::new();
    stdin.read_line(&mut line).ok();
    line.trim().to_string()
}

fn prompt_yes_no<R: BufRead, W: Write>(
    stdin: &mut R,
    stdout: &mut W,
    prompt: &str,
    default: bool,
) -> bool {
    loop {
        let input = prompt_line(stdin, stdout, prompt).to_lowercase();
        if input.is_empty() {
            return default;
        }
        match input.as_str() {
            "y" | "yes" => return true,
            "n" | "no" => return false,
            _ => {
                writeln!(stdout, "⚠  Enter yes or no.").ok();
            }
        }
    }
}

fn normalize_api_url(value: &str) -> String {
    let normalized = value.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        default_api_url()
    } else {
        normalized
    }
}

fn run_github_login(api_url: &str) -> Result<CliLoginResult> {
    let api_url = normalize_api_url(api_url);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let start_url = format!("{}/api/auth/cli/start", api_url);
    let response = client
        .post(&start_url)
        .json(&serde_json::json!({ "name": "tokenboard CLI" }))
        .send()
        .map_err(|error| github_login_network_error(&api_url, "starting GitHub login", error))?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!(
            "Tokenboard server returned {} while starting GitHub login at {}.",
            status.as_u16(),
            start_url
        );
    }

    let login: CliLoginStart = response
        .json()
        .context("Failed to parse GitHub login start response")?;

    println!();
    if open_browser(&login.login_url) {
        println!("Opened GitHub login in your browser.");
        println!("If it did not open, visit:");
    } else {
        println!("Open this URL to sign in with GitHub:");
    }
    println!("{}", login.login_url);
    print!("Waiting for GitHub login...");
    io::stdout().flush().ok();

    let deadline = Instant::now() + Duration::from_secs(login.expires_in.max(1));
    let poll_interval = Duration::from_secs(login.poll_interval.max(1));
    let poll_url = format!(
        "{}/api/auth/cli/poll?code={}",
        api_url,
        url_encode_component(&login.code)
    );

    loop {
        if Instant::now() >= deadline {
            println!();
            anyhow::bail!("Timed out waiting for GitHub login");
        }

        let response = client
            .get(&poll_url)
            .send()
            .map_err(|error| github_login_network_error(&api_url, "polling GitHub login", error))?;
        let status = response.status();

        if status == reqwest::StatusCode::ACCEPTED {
            print!(".");
            io::stdout().flush().ok();
            std::thread::sleep(poll_interval);
            continue;
        }

        if !status.is_success() {
            println!();
            anyhow::bail!(
                "Tokenboard server returned {} while polling GitHub login at {}.",
                status.as_u16(),
                poll_url
            );
        }

        let poll: CliLoginPoll = response
            .json()
            .context("Failed to parse GitHub login poll response")?;
        if poll.status == "complete" {
            println!();
            let token = poll
                .token
                .filter(|value| !value.trim().is_empty())
                .context("GitHub login did not return CLI credentials")?;
            let user = poll.user.context("GitHub login did not return a user")?;
            return Ok(CliLoginResult { token, user });
        }

        print!(".");
        io::stdout().flush().ok();
        std::thread::sleep(poll_interval);
    }
}

fn github_login_network_error(api_url: &str, action: &str, error: reqwest::Error) -> anyhow::Error {
    anyhow::anyhow!("Could not reach Tokenboard server {api_url} while {action}: {error}")
}

fn open_browser(url: &str) -> bool {
    let status = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).status()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .status()
    } else {
        std::process::Command::new("xdg-open").arg(url).status()
    };

    status.map(|status| status.success()).unwrap_or(false)
}

fn url_encode_component(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for byte in value.bytes() {
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

// ============================================================================
// Config file I/O
// ============================================================================

/// Load config from ~/.tokenboard/config.toml, merging env vars.
fn load_config_with_setup(allow_setup: bool) -> Result<sync::SyncConfig> {
    // Priority order:
    // 1. Environment variables (highest)
    // 2. .env files (current dir + ~/.tokenboard/.env)
    // 3. ~/.tokenboard/config.toml
    // 4. Interactive setup wizard (if still missing required fields)

    let env_api_url = env::var("TOKENBOARD_API_URL").ok();
    let env_api_token = env::var("TOKENBOARD_API_TOKEN")
        .ok()
        .or_else(|| env::var("TOKENBOARD_API_KEY").ok());
    let env_github_username = env::var("TOKENBOARD_GITHUB_USERNAME")
        .ok()
        .or_else(|| env::var("TOKENBOARD_USERNAME").ok());
    let env_display_name = env::var("TOKENBOARD_DISPLAY_NAME").ok();

    // Try .env files (override config file, not already-exported env vars)
    load_env_files();

    let file_cfg = load_config_file();

    let api_url = env_api_url
        .or_else(|| env::var("TOKENBOARD_API_URL").ok())
        .unwrap_or_else(|| {
            if file_cfg.api_url.is_empty() {
                default_api_url()
            } else {
                file_cfg.api_url.clone()
            }
        });

    let mut api_token = env_api_token
        .or_else(|| {
            env::var("TOKENBOARD_API_TOKEN")
                .ok()
                .or_else(|| env::var("TOKENBOARD_API_KEY").ok())
        })
        .unwrap_or_else(|| file_cfg.api_token.clone());

    let mut github_username = env_github_username
        .or_else(|| {
            env::var("TOKENBOARD_GITHUB_USERNAME")
                .ok()
                .or_else(|| env::var("TOKENBOARD_USERNAME").ok())
        })
        .unwrap_or_else(|| file_cfg.github_username.clone());
    let mut display_name = env_display_name
        .or_else(|| env::var("TOKENBOARD_DISPLAY_NAME").ok())
        .unwrap_or_else(|| file_cfg.display_name.clone());

    // If still missing required fields, run interactive setup
    if api_token.trim().is_empty() || github_username.trim().is_empty() {
        if !allow_setup {
            anyhow::bail!(
                "Missing Tokenboard config. Run `tokenboard setup` before scheduled sync."
            );
        }
        eprintln!();
        let mut cfg = file_cfg.clone();
        cfg.api_url = api_url.clone();
        cfg.api_token = api_token;
        cfg.github_username = github_username;
        cfg.display_name = display_name;
        cfg = run_setup(&cfg)?;
        save_config_file(&cfg)?;
        api_token = cfg.api_token;
        github_username = cfg.github_username;
        display_name = cfg.display_name;
    }

    if api_token.trim().is_empty() {
        anyhow::bail!("TOKENBOARD_API_TOKEN cannot be empty");
    }
    if github_username.trim().is_empty() {
        anyhow::bail!("TOKENBOARD_GITHUB_USERNAME cannot be empty");
    }
    api_token = api_token.trim().to_string();
    github_username = github_username.trim().to_string();
    display_name = display_name.trim().to_string();

    if display_name.trim().is_empty() {
        display_name = github_username.clone();
    }
    let api_url = {
        let normalized = api_url.trim().trim_end_matches('/').to_string();
        if normalized.is_empty() {
            default_api_url()
        } else {
            normalized
        }
    };

    Ok(sync::SyncConfig {
        api_url,
        api_token,
        display_name,
        github_username,
    })
}

fn load_update_config() -> updater::UpdateConfig {
    load_env_files();
    let file_cfg = load_config_file();
    let auto_update = env::var("TOKENBOARD_AUTO_UPDATE")
        .ok()
        .and_then(|value| parse_bool_env(&value))
        .unwrap_or(file_cfg.auto_update);
    let repo = env::var("TOKENBOARD_UPDATE_REPO")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            if file_cfg.update_repo.trim().is_empty() {
                default_update_repo()
            } else {
                file_cfg.update_repo.clone()
            }
        });
    let github_token = env::var("TOKENBOARD_UPDATE_GITHUB_TOKEN")
        .ok()
        .unwrap_or_else(|| file_cfg.update_github_token.clone());

    updater::UpdateConfig {
        auto_update,
        repo: repo.trim().to_string(),
        github_token: github_token.trim().to_string(),
    }
}

fn load_env_files() {
    let _ = dotenv::from_current_dir();
    if let Some(home) = dirs::home_dir() {
        let _ = dotenv::from_path(&home.join(".tokenboard").join(".env"));
    }
}

fn parse_bool_env(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

fn load_config_file() -> ConfigFile {
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(cfg) => return cfg,
                Err(e) => eprintln!("⚠  Failed to parse {}: {}\n", path.display(), e),
            },
            Err(e) => eprintln!("⚠  Could not read {}: {}\n", path.display(), e),
        }
    }
    ConfigFile::default()
}

fn ensure_persistent_config_for_autosync() -> Result<()> {
    let file_cfg = load_config_file();
    let home_env = config_dir().join(".env");
    let (home_env_has_token, home_env_has_username) = home_env_config_presence(&home_env);
    let has_token = !file_cfg.api_token.trim().is_empty() || home_env_has_token;
    let has_username = !file_cfg.github_username.trim().is_empty() || home_env_has_username;

    if has_token && has_username {
        return Ok(());
    }

    eprintln!("Automatic sync needs saved credentials in ~/.tokenboard/config.toml.");
    eprintln!("Starting setup now so scheduled sync can run without a terminal.\n");
    let cfg = run_setup(&file_cfg)?;
    save_config_file(&cfg)?;
    Ok(())
}

fn config_dir() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".tokenboard")
}

fn home_env_config_presence(path: &std::path::Path) -> (bool, bool) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return (false, false);
    };

    let mut has_token = false;
    let mut has_username = false;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            continue;
        }
        match key {
            "TOKENBOARD_API_TOKEN" | "TOKENBOARD_API_KEY" => has_token = true,
            "TOKENBOARD_GITHUB_USERNAME" | "TOKENBOARD_USERNAME" => has_username = true,
            _ => {}
        }
    }

    (has_token, has_username)
}

fn save_config_file(cfg: &ConfigFile) -> Result<()> {
    let path = config_path();
    save_config_file_at(&path, cfg)?;
    eprintln!("✅ Config saved to {}", path.display());
    Ok(())
}

fn save_config_file_at(path: &Path, cfg: &ConfigFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create ~/.tokenboard directory")?;
    }
    let content = toml::to_string_pretty(cfg).context("Failed to serialize config")?;
    write_private_config_file(path, content.as_bytes()).context("Failed to write config file")?;
    Ok(())
}

fn write_private_config_file(path: &Path, content: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(path)
        .with_context(|| format!("Failed to open {} for writing", path.display()))?;
    set_private_config_permissions(&file, path)?;
    file.set_len(0)
        .with_context(|| format!("Failed to truncate {}", path.display()))?;
    file.seek(SeekFrom::Start(0))
        .with_context(|| format!("Failed to seek {}", path.display()))?;
    file.write_all(content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to sync {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_config_permissions(file: &fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = file
        .metadata()
        .with_context(|| format!("Failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    file.set_permissions(permissions)
        .with_context(|| format!("Failed to make {} private", path.display()))
}

#[cfg(not(unix))]
fn set_private_config_permissions(_file: &fs::File, _path: &Path) -> Result<()> {
    // Windows needs ACLs or keychain storage; std::fs exposes no 0600 equivalent.
    Ok(())
}

fn config_path() -> std::path::PathBuf {
    config_dir().join("config.toml")
}

// ============================================================================
// Output helpers
// ============================================================================

fn describe_filters(clients: Option<&Vec<String>>, date_range: Option<(i64, i64)>) -> String {
    let mut parts: Vec<String> = Vec::new();

    if let Some(cs) = clients {
        parts.push(format!("({})", cs.join(", ")));
    }
    if let Some((since, until)) = date_range {
        let since_str = ms_to_date(since);
        let until_str = ms_to_date(until);
        if since == until {
            parts.push(format!("on {}", since_str));
        } else {
            parts.push(format!("{} → {}", since_str, until_str));
        }
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" {}", parts.join(" "))
    }
}

fn print_no_data_found(clients: &Option<Vec<String>>, date_range: &Option<(i64, i64)>) {
    let ctx = describe_filters(clients.as_ref(), *date_range);
    if ctx.is_empty() {
        println!("No token usage data found.");
    } else {
        println!("No token usage data found{}.", ctx);
    }
    let agents = clients::all_ids();
    println!("Supported agents: {}", agents.join(", "));
    println!("Have you used any AI coding agents? If so, their session files may be in a");
    println!("different location.");
}

fn print_scan_summary(records: &[scanner::TokenUsage]) {
    println!("Found {} token usage records\n", records.len());

    let days = scanner::aggregate(records);
    println!("Aggregated into {} days of activity:\n", days.len());

    let mut total_tokens: i64 = 0;
    let mut total_cost: f64 = 0.0;

    for (date, day) in &days {
        println!("📅 {}", date);
        println!(
            "   Tokens: {:>12}  Cost: ${:.2}",
            format_num(day.total_tokens),
            day.total_cost
        );
        println!(
            "   Input: {:>13}  Output: {:>12}",
            format_num(day.input_tokens),
            format_num(day.output_tokens)
        );
        println!(
            "   Cache read: {:>8}  Write: {:>11}",
            format_num(day.cache_read_tokens),
            format_num(day.cache_write_tokens)
        );

        if !day.models.is_empty() {
            println!("   Models:");
            for (model_key, m) in &day.models {
                // model_key is "model_name|agent"
                println!(
                    "     {} via {} — {} tokens (in:{}, out:{}, cache_r:{}, cache_w:{})",
                    model_key,
                    m.source,
                    format_num(m.tokens),
                    format_num(m.input_tokens),
                    format_num(m.output_tokens),
                    format_num(m.cache_read_tokens),
                    format_num(m.cache_write_tokens),
                );
            }
        }
        if !day.clients.is_empty() {
            println!("   Clients:");
            for (client, c) in &day.clients {
                println!(
                    "     {} ({} tokens, ${:.2})",
                    client,
                    format_num(c.tokens),
                    c.cost
                );
            }
        }
        println!();

        total_tokens += day.total_tokens;
        total_cost += day.total_cost;
    }

    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!(
        "Total: {} tokens, ${:.2} across {} agents",
        format_num(total_tokens),
        total_cost,
        days.values()
            .flat_map(|d| d.clients.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );

    let all_models: std::collections::BTreeSet<_> =
        days.values().flat_map(|d| d.models.keys()).collect();
    println!("Models used: {}", all_models.len());
    println!(
        "Day range: {} to {}",
        days.keys().next().map(|s| s.as_str()).unwrap_or("N/A"),
        days.keys().next_back().map(|s| s.as_str()).unwrap_or("N/A"),
    );
}

fn format_num(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn ms_to_date(ms: i64) -> String {
    use chrono::TimeZone;
    let secs = ms / 1000;
    let nsecs = ((ms % 1000) * 1_000_000) as u32;
    chrono::Utc
        .timestamp_opt(secs, nsecs)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod setup_tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Cursor;

    #[test]
    fn default_api_url_uses_hosted_tokenboard() {
        assert_eq!(default_api_url(), "https://tokenboard.net");
    }

    #[test]
    fn api_url_prompt_keeps_hosted_default_on_empty_yes() {
        let mut input = Cursor::new(b"\n".to_vec());
        let mut output = Vec::new();

        let api_url = prompt_api_url(&mut input, &mut output, "").unwrap();

        assert_eq!(api_url, "https://tokenboard.net");
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Use https://tokenboard.net as your Tokenboard server? [Y/n]:"));
    }

    #[test]
    fn api_url_prompt_accepts_custom_url_after_no() {
        let mut input = Cursor::new(b"n\nhttps://tokenboard.example.com///\n".to_vec());
        let mut output = Vec::new();

        let api_url = prompt_api_url(&mut input, &mut output, "").unwrap();

        assert_eq!(api_url, "https://tokenboard.example.com");
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("Tokenboard server URL:"));
    }

    #[test]
    fn setup_stops_on_github_auth_failure_without_prompting_for_token() {
        let existing = ConfigFile::default();
        let mut input = Cursor::new(b"\n\n".to_vec());
        let mut output = Vec::new();
        let called = Cell::new(false);

        let result = run_setup_with_io(&existing, &mut input, &mut output, |api_url| {
            called.set(true);
            assert_eq!(api_url, "https://tokenboard.net");
            anyhow::bail!(
                "Tokenboard server returned 400 while starting GitHub login at https://tokenboard.net/api/auth/cli/start."
            );
        });

        assert!(called.get());
        let error = result.unwrap_err().to_string();
        assert!(error.contains("GitHub sign-in failed for https://tokenboard.net"));
        assert!(!error.contains("Tokenboard API token"));
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("Tokenboard API token"));
    }

    #[test]
    fn setup_cancelled_message_is_used_when_required_github_auth_is_declined() {
        let existing = ConfigFile::default();
        let mut input = Cursor::new(b"\nn\n".to_vec());
        let mut output = Vec::new();
        let called = Cell::new(false);

        let result = run_setup_with_io(&existing, &mut input, &mut output, |_api_url| {
            called.set(true);
            unreachable!("login should not start when the user declines required GitHub auth");
        });

        assert!(!called.get());
        assert_eq!(result.unwrap_err().to_string(), setup_cancelled_message());
        let output = String::from_utf8(output).unwrap();
        assert!(!output.contains("Tokenboard API token"));
    }

    #[test]
    fn setup_login_failure_message_strips_html_response_bodies() {
        let error = anyhow::anyhow!(
            "Server returned 400 while starting GitHub login: <!DOCTYPE html><html><body><pre>stack trace</pre></body></html>"
        );

        let message = setup_login_failure_message("https://tokenboard.net", &error);

        assert!(message.contains("Tokenboard server returned an unreadable error response."));
        assert!(!message.contains("<html>"));
        assert!(!message.contains("stack trace"));
    }
}

#[cfg(test)]
mod config_file_tests {
    use super::*;

    #[test]
    fn save_config_file_at_writes_config_content() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("nested").join("config.toml");
        let cfg = ConfigFile {
            api_token: "secret-token".to_string(),
            github_username: "octocat".to_string(),
            display_name: "Octocat".to_string(),
            ..ConfigFile::default()
        };

        save_config_file_at(&path, &cfg).unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("api_token = \"secret-token\""));
        assert!(content.contains("github_username = \"octocat\""));
    }

    #[cfg(unix)]
    #[test]
    fn save_config_file_at_sets_private_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "api_token = \"old\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let cfg = ConfigFile {
            api_token: "secret-token".to_string(),
            github_username: "octocat".to_string(),
            ..ConfigFile::default()
        };

        save_config_file_at(&path, &cfg).unwrap();

        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

// ============================================================================
// Dotenv helper (inline, no crate needed)
// ============================================================================

mod dotenv {
    use std::path::Path;
    pub fn from_current_dir() -> Result<(), std::io::Error> {
        from_path(Path::new(".env"))
    }
    pub fn from_path(path: &Path) -> Result<(), std::io::Error> {
        if !path.exists() {
            return Ok(());
        }
        let content = std::fs::read_to_string(path)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if std::env::var(key).is_err() {
                    std::env::set_var(key, value);
                }
            }
        }
        Ok(())
    }
}
