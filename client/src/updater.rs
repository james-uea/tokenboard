//! Self-update support backed by GitHub Releases.

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_REPO: &str = "james-uea/tokenboard";
const USER_AGENT_VALUE: &str = "tokenboard-updater";
const KEEP_OLD_VERSIONS: usize = 2;

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub auto_update: bool,
    pub repo: String,
    pub github_token: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            auto_update: true,
            repo: DEFAULT_REPO.to_string(),
            github_token: String::new(),
        }
    }
}

#[derive(Debug)]
pub enum CheckOutcome {
    UpToDate {
        current: String,
        latest: String,
    },
    UpdateAvailable {
        current: String,
        latest: String,
        asset_name: String,
    },
}

#[derive(Debug)]
pub enum InstallOutcome {
    AlreadyCurrent {
        current: String,
        latest: String,
    },
    Installed {
        version: String,
        backup: PathBuf,
    },
    #[cfg_attr(not(windows), allow(dead_code))]
    StagedForWindows {
        version: String,
        helper: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    url: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

pub fn check(config: &UpdateConfig) -> Result<CheckOutcome> {
    let client = http_client()?;
    let release = fetch_latest_release(&client, config)?;
    let latest = parse_release_version(&release.tag_name)?;
    let current = current_version()?;

    if latest <= current {
        return Ok(CheckOutcome::UpToDate {
            current: current.to_string(),
            latest: latest.to_string(),
        });
    }

    let asset =
        select_asset(&release.assets).context("No matching release asset for this OS/arch")?;
    Ok(CheckOutcome::UpdateAvailable {
        current: current.to_string(),
        latest: latest.to_string(),
        asset_name: asset.name.clone(),
    })
}

pub fn install_latest(config: &UpdateConfig) -> Result<InstallOutcome> {
    let client = http_client()?;
    let release = fetch_latest_release(&client, config)?;
    let latest = parse_release_version(&release.tag_name)?;
    let current = current_version()?;

    if latest <= current {
        return Ok(InstallOutcome::AlreadyCurrent {
            current: current.to_string(),
            latest: latest.to_string(),
        });
    }

    let asset =
        select_asset(&release.assets).context("No matching release asset for this OS/arch")?;
    let root = tokenboard_dir()?;
    let staging = root.join("update-staging");
    fs::create_dir_all(&staging).context("Failed to create update staging directory")?;
    let staged = staging.join(staged_file_name(&asset.name, &latest));

    let bytes = download_asset(&client, config, asset)?;
    verify_download(&client, config, &bytes, &release.assets, asset)
        .with_context(|| format!("Failed to verify {}", asset.name))?;
    write_staged_binary(&staged, &bytes)?;

    let exe = std::env::current_exe().context("Failed to locate current tokenboard executable")?;
    let backup = backup_path(&root, &latest, &exe)?;

    install_staged(&staged, &exe, &backup, &root, &latest)
}

pub fn default_repo() -> &'static str {
    DEFAULT_REPO
}

fn current_version() -> Result<Version> {
    parse_version(env!("CARGO_PKG_VERSION"))
}

fn parse_release_version(tag: &str) -> Result<Version> {
    parse_version(tag.trim().trim_start_matches('v').trim_start_matches('V'))
        .with_context(|| format!("Invalid release version tag '{tag}'"))
}

fn parse_version(raw: &str) -> Result<Version> {
    Version::parse(raw).with_context(|| format!("Invalid semver version '{raw}'"))
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(USER_AGENT_VALUE)
        .build()
        .context("Failed to create updater HTTP client")
}

fn fetch_latest_release(client: &Client, config: &UpdateConfig) -> Result<GithubRelease> {
    let repo = normalize_repo(&config.repo)?;
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut request = client
        .get(&url)
        .header(ACCEPT, "application/vnd.github+json")
        .header(USER_AGENT, USER_AGENT_VALUE);
    if let Some(token) = auth_token(config) {
        request = request.header(AUTHORIZATION, format!("Bearer {token}"));
    }

    let response = request
        .send()
        .with_context(|| format!("Failed to fetch latest release from {url}"))?;
    ensure_success(response, "fetch latest release")?
        .json()
        .context("Failed to parse latest release response")
}

fn normalize_repo(repo: &str) -> Result<String> {
    let repo = repo.trim().trim_matches('/');
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2 || parts.iter().any(|part| part.trim().is_empty()) {
        bail!("Update repo must be in owner/name form, got '{repo}'");
    }
    Ok(format!("{}/{}", parts[0], parts[1]))
}

fn auth_token(config: &UpdateConfig) -> Option<&str> {
    let token = config.github_token.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn ensure_success(response: Response, action: &str) -> Result<Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response
        .text()
        .unwrap_or_else(|_| "unknown error".to_string());
    bail!(
        "GitHub returned {} while trying to {}: {}",
        status,
        action,
        body
    );
}

fn select_asset(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    let expected = platform_asset_names()?;
    expected
        .iter()
        .find_map(|name| assets.iter().find(|asset| asset.name == *name))
}

fn platform_asset_names() -> Option<&'static [&'static str]> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Some(&[
            "tokenboard-x86_64-unknown-linux-musl",
            "tokenboard-x86_64-unknown-linux-gnu",
        ]),
        ("macos", "aarch64") => Some(&["tokenboard-aarch64-apple-darwin"]),
        ("macos", "x86_64") => Some(&["tokenboard-x86_64-apple-darwin"]),
        ("windows", "x86_64") => Some(&["tokenboard-x86_64-pc-windows-msvc.exe"]),
        _ => None,
    }
}

fn download_asset(client: &Client, config: &UpdateConfig, asset: &GithubAsset) -> Result<Vec<u8>> {
    let response = if let Some(token) = auth_token(config) {
        client
            .get(&asset.url)
            .header(ACCEPT, "application/octet-stream")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(USER_AGENT, USER_AGENT_VALUE)
            .send()
            .with_context(|| format!("Failed to download release asset {}", asset.name))?
    } else {
        client
            .get(&asset.browser_download_url)
            .header(USER_AGENT, USER_AGENT_VALUE)
            .send()
            .with_context(|| format!("Failed to download release asset {}", asset.name))?
    };

    Ok(ensure_success(response, "download release asset")?
        .bytes()
        .context("Failed to read release asset body")?
        .to_vec())
}

fn verify_download(
    client: &Client,
    config: &UpdateConfig,
    bytes: &[u8],
    assets: &[GithubAsset],
    asset: &GithubAsset,
) -> Result<()> {
    if let Some(expected) = asset.digest.as_deref().and_then(parse_sha256_digest) {
        return verify_sha256(bytes, &expected);
    }

    let checksum_name = format!("{}.sha256", asset.name);
    if let Some(checksum_asset) = assets
        .iter()
        .find(|candidate| candidate.name == checksum_name)
    {
        let checksum = download_asset(client, config, checksum_asset)?;
        let checksum = String::from_utf8(checksum).context("Checksum asset was not valid UTF-8")?;
        let expected = parse_sha256_file(&checksum).with_context(|| {
            format!("Checksum asset {} did not contain a SHA-256", checksum_name)
        })?;
        return verify_sha256(bytes, &expected);
    }

    Ok(())
}

fn parse_sha256_digest(raw: &str) -> Option<String> {
    raw.strip_prefix("sha256:")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
}

fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        bail!("SHA-256 mismatch: expected {expected}, got {actual}");
    }
}

fn parse_sha256_file(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .find(|part| part.len() == 64 && part.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|part| part.to_ascii_lowercase())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn write_staged_binary(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("Failed to create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("Failed to sync {}", path.display()))?;
    set_executable(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("Failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("Failed to mark {} executable", path.display()))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn install_staged(
    staged: &Path,
    exe: &Path,
    backup: &Path,
    root: &Path,
    latest: &Version,
) -> Result<InstallOutcome> {
    #[cfg(windows)]
    {
        let helper = write_windows_helper(staged, exe, backup, root)?;
        spawn_windows_helper(&helper)?;
        return Ok(InstallOutcome::StagedForWindows {
            version: latest.to_string(),
            helper,
        });
    }

    #[cfg(not(windows))]
    {
        install_unix(staged, exe, backup)?;
        prune_old_versions(&root.join("versions"))?;
        Ok(InstallOutcome::Installed {
            version: latest.to_string(),
            backup: backup.to_path_buf(),
        })
    }
}

#[cfg(not(windows))]
fn install_unix(staged: &Path, exe: &Path, backup: &Path) -> Result<()> {
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    fs::rename(exe, backup).with_context(|| {
        format!(
            "Failed to move current executable {} to {}",
            exe.display(),
            backup.display()
        )
    })?;

    if let Err(error) = fs::rename(staged, exe) {
        let _ = fs::rename(backup, exe);
        bail!(
            "Failed to install update at {}; restored previous executable: {}",
            exe.display(),
            error
        );
    }

    Ok(())
}

#[cfg(windows)]
fn write_windows_helper(staged: &Path, exe: &Path, backup: &Path, root: &Path) -> Result<PathBuf> {
    if let Some(parent) = backup.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let helper = root
        .join("update-staging")
        .join("apply-tokenboard-update.cmd");
    let log = root.join("update.log");
    let versions = root.join("versions");
    let script = windows_helper_script(staged, exe, backup, &versions, &log);
    fs::write(&helper, script).with_context(|| format!("Failed to write {}", helper.display()))?;
    Ok(helper)
}

#[cfg(windows)]
fn spawn_windows_helper(helper: &Path) -> Result<()> {
    use std::process::Command;

    Command::new("cmd")
        .args(["/C", "start", "", "/min"])
        .arg(helper)
        .spawn()
        .with_context(|| format!("Failed to spawn {}", helper.display()))?;
    Ok(())
}

#[cfg(windows)]
fn windows_helper_script(
    staged: &Path,
    exe: &Path,
    backup: &Path,
    versions: &Path,
    log: &Path,
) -> String {
    format!(
        "@echo off\r\n\
         setlocal\r\n\
         set \"STAGED={staged}\"\r\n\
         set \"EXE={exe}\"\r\n\
         set \"BACKUP={backup}\"\r\n\
         set \"VERSIONS={versions}\"\r\n\
         set \"LOG={log}\"\r\n\
         echo Applying tokenboard update > \"%LOG%\"\r\n\
         timeout /t 2 /nobreak >nul\r\n\
         move /Y \"%EXE%\" \"%BACKUP%\" >> \"%LOG%\" 2>>&1\r\n\
         if errorlevel 1 exit /b 1\r\n\
         move /Y \"%STAGED%\" \"%EXE%\" >> \"%LOG%\" 2>>&1\r\n\
         if errorlevel 1 (\r\n\
         move /Y \"%BACKUP%\" \"%EXE%\" >> \"%LOG%\" 2>>&1\r\n\
         exit /b 1\r\n\
         )\r\n\
         powershell -NoProfile -ExecutionPolicy Bypass -Command \"Get-ChildItem -LiteralPath '%VERSIONS%' -Filter 'tokenboard-*' | Sort-Object LastWriteTime -Descending | Select-Object -Skip 2 | Remove-Item -Force\" >> \"%LOG%\" 2>>&1\r\n\
         exit /b 0\r\n",
        staged = staged.display(),
        exe = exe.display(),
        backup = backup.display(),
        versions = versions.display(),
        log = log.display()
    )
}

fn tokenboard_dir() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".tokenboard"))
        .context("Could not determine your home directory")
}

fn backup_path(root: &Path, latest: &Version, exe: &Path) -> Result<PathBuf> {
    let versions = root.join("versions");
    fs::create_dir_all(&versions)
        .with_context(|| format!("Failed to create {}", versions.display()))?;
    let ext = exe.extension().and_then(OsStr::to_str).unwrap_or("");
    let suffix = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let file = if ext.is_empty() {
        format!("tokenboard-before-{}-{}", latest, suffix)
    } else {
        format!("tokenboard-before-{}-{}.{}", latest, suffix, ext)
    };
    Ok(versions.join(file))
}

fn staged_file_name(asset_name: &str, latest: &Version) -> String {
    format!("{}-{}", latest, asset_name)
}

fn prune_old_versions(versions_dir: &Path) -> Result<()> {
    prune_old_versions_with_limit(versions_dir, KEEP_OLD_VERSIONS)
}

fn prune_old_versions_with_limit(versions_dir: &Path, keep: usize) -> Result<()> {
    if !versions_dir.exists() {
        return Ok(());
    }

    let mut entries = fs::read_dir(versions_dir)
        .with_context(|| format!("Failed to read {}", versions_dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            Some((entry.path(), metadata.modified().ok()?))
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| b.1.cmp(&a.1));
    for (path, _) in entries.into_iter().skip(keep) {
        fs::remove_file(&path).with_context(|| format!("Failed to remove {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            url: "https://api.github.com/assets/1".to_string(),
            browser_download_url: "https://github.com/download".to_string(),
            digest: None,
        }
    }

    #[test]
    fn parses_v_prefixed_release_versions() {
        assert_eq!(
            parse_release_version("v1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
        assert_eq!(
            parse_release_version("1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
    }

    #[test]
    fn selects_current_platform_asset_when_supported() {
        let expected = platform_asset_names();
        let mut assets = vec![asset("other")];
        if let Some(expected) = expected {
            assets.push(asset(expected[0]));
            assert_eq!(select_asset(&assets).unwrap().name, expected[0]);
        } else {
            assert!(select_asset(&assets).is_none());
        }
    }

    #[test]
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    fn linux_prefers_musl_release_asset() {
        let assets = vec![
            asset("tokenboard-x86_64-unknown-linux-gnu"),
            asset("tokenboard-x86_64-unknown-linux-musl"),
        ];

        assert_eq!(
            select_asset(&assets).unwrap().name,
            "tokenboard-x86_64-unknown-linux-musl"
        );
    }

    #[test]
    fn verifies_matching_sha256_digest() {
        let bytes = b"tokenboard";
        let digest = sha256_hex(bytes);
        verify_sha256(bytes, &digest).unwrap();
        assert!(verify_sha256(
            bytes,
            "0000000000000000000000000000000000000000000000000000000000000000"
        )
        .is_err());
    }

    #[test]
    fn parses_github_sha256_digest_format() {
        let digest = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(
            parse_sha256_digest(digest).unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(parse_sha256_digest("md5:abc").is_none());
    }

    #[test]
    fn parses_sha256_companion_file() {
        assert_eq!(
            parse_sha256_file(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  tokenboard"
            )
            .unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        );
        assert!(parse_sha256_file("not-a-checksum").is_none());
    }

    #[test]
    fn prunes_old_versions_to_limit() {
        let dir = tempfile::tempdir().unwrap();
        for idx in 0..4 {
            let path = dir.path().join(format!("tokenboard-{idx}"));
            fs::write(&path, idx.to_string()).unwrap();
            thread::sleep(Duration::from_millis(5));
        }

        prune_old_versions_with_limit(dir.path(), 2).unwrap();
        let remaining = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 2);
    }

    #[test]
    fn default_update_config_is_enabled_for_default_repo() {
        let config = UpdateConfig::default();
        assert!(config.auto_update);
        assert_eq!(config.repo, "james-uea/tokenboard");
        assert!(config.github_token.is_empty());
    }

    #[test]
    fn backup_path_keeps_exe_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = backup_path(
            dir.path(),
            &Version::new(1, 2, 3),
            Path::new("tokenboard.exe"),
        )
        .unwrap();
        assert_eq!(path.extension().and_then(OsStr::to_str), Some("exe"));
    }

    #[test]
    fn prune_ignores_directories() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("tokenboard-old"), "old").unwrap();
        prune_old_versions_with_limit(dir.path(), 2).unwrap();
        assert!(dir.path().join("nested").exists());
    }

    #[test]
    fn unsupported_asset_list_returns_none() {
        assert!(select_asset(&[asset("tokenboard-unsupported")]).is_none());
    }

    #[test]
    fn modified_time_sort_has_stable_limit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("tokenboard-a"), "a").unwrap();
        thread::sleep(Duration::from_millis(5));
        fs::write(dir.path().join("tokenboard-b"), "b").unwrap();
        prune_old_versions_with_limit(dir.path(), 1).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("tokenboard-b")).unwrap(),
            "b"
        );
        assert!(!dir.path().join("tokenboard-a").exists());
    }
}
