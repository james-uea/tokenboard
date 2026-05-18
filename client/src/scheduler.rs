//! OS-native scheduling for automatic tokenboard sync.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const INTERVAL_HOURS: u64 = 3;
const SYNC_ARGS: [&str; 2] = ["sync", "--no-setup"];

pub fn install() -> Result<String> {
    platform::install()
}

pub fn uninstall() -> Result<String> {
    platform::uninstall()
}

pub fn status() -> Result<String> {
    platform::status()
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("Failed to locate the tokenboard executable")
}

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("Could not determine your home directory")
}

fn tokenboard_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".tokenboard"))
}

fn ensure_tokenboard_dir() -> Result<()> {
    std::fs::create_dir_all(tokenboard_dir()?).context("Failed to create ~/.tokenboard")
}

fn log_path(name: &str) -> Result<PathBuf> {
    Ok(tokenboard_dir()?.join(name))
}

fn latest_log_update() -> Result<Option<String>> {
    let latest = ["autosync.log", "autosync.err"]
        .iter()
        .filter_map(|name| {
            let path = log_path(name).ok()?;
            std::fs::metadata(path).ok()?.modified().ok()
        })
        .max();

    Ok(latest.map(format_system_time))
}

fn format_system_time(time: SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = time.into();
    datetime.format("%Y-%m-%d %H:%M:%S %Z").to_string()
}

fn append_last_log_update(mut message: String) -> Result<String> {
    if let Some(updated_at) = latest_log_update()? {
        message.push_str(&format!(" Last log update: {updated_at}."));
    }
    Ok(message)
}

fn run_command(command: &mut Command, action: &str) -> Result<()> {
    let debug = format!("{command:?}");
    let output = command
        .output()
        .with_context(|| format!("Failed to run command for {action}: {debug}"))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    anyhow::bail!("Failed to {action}: {detail}");
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn sh_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn without_marked_block(existing: &str, begin: &str, end: &str) -> String {
    let mut kept = Vec::new();
    let mut in_managed_block = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == begin {
            in_managed_block = true;
            continue;
        }
        if trimmed == end {
            in_managed_block = false;
            continue;
        }
        if !in_managed_block {
            kept.push(line);
        }
    }

    let trimmed = kept.join("\n").trim_end().to_string();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    const LABEL: &str = "com.tokenboard.autosync";
    const PLIST_NAME: &str = "com.tokenboard.autosync.plist";
    const CALENDAR_HOURS: [u8; 8] = [0, 3, 6, 9, 12, 15, 18, 21];

    pub fn install() -> Result<String> {
        let exe = current_exe()?;
        ensure_tokenboard_dir()?;

        let plist_path = launch_agents_dir()?.join(PLIST_NAME);
        let plist = launchd_plist(&exe)?;
        std::fs::write(&plist_path, plist)
            .with_context(|| format!("Failed to write {}", plist_path.display()))?;

        let _ = Command::new("launchctl")
            .arg("unload")
            .arg(&plist_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        run_command(
            Command::new("launchctl").arg("load").arg(&plist_path),
            "load tokenboard LaunchAgent",
        )?;

        Ok(format!(
            "Automatic sync installed with macOS LaunchAgent at {}. It runs every {} hours and at login. Logs: {}",
            plist_path.display(),
            INTERVAL_HOURS,
            log_path("autosync.log")?.display()
        ))
    }

    pub fn uninstall() -> Result<String> {
        let plist_path = launch_agents_dir()?.join(PLIST_NAME);
        let _ = Command::new("launchctl")
            .arg("unload")
            .arg(&plist_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if plist_path.exists() {
            std::fs::remove_file(&plist_path)
                .with_context(|| format!("Failed to remove {}", plist_path.display()))?;
        }

        Ok("Automatic sync removed from macOS LaunchAgents.".to_string())
    }

    pub fn status() -> Result<String> {
        let plist_path = launch_agents_dir()?.join(PLIST_NAME);
        let output = Command::new("launchctl").args(["list", LABEL]).output();

        match output {
            Ok(output) if output.status.success() => append_last_log_update(format!(
                "Automatic sync is installed and loaded via macOS LaunchAgent at {}.",
                plist_path.display()
            )),
            _ if plist_path.exists() => append_last_log_update(format!(
                "Automatic sync is installed at {}, but launchd does not report it as loaded. Run `tokenboard autosync install` to reload it.",
                plist_path.display()
            )),
            _ => Ok("Automatic sync is not installed.".to_string()),
        }
    }

    fn launch_agents_dir() -> Result<PathBuf> {
        let dir = home_dir()?.join("Library").join("LaunchAgents");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;
        Ok(dir)
    }

    pub(super) fn launchd_plist(exe: &Path) -> Result<String> {
        let stdout = log_path("autosync.log")?;
        let stderr = log_path("autosync.err")?;
        Ok(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{exe}</string>
    <string>{arg0}</string>
    <string>{arg1}</string>
  </array>
  <key>StartCalendarInterval</key>
  <array>
{calendar_intervals}  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>StandardOutPath</key>
  <string>{stdout}</string>
  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>
"#,
            label = LABEL,
            exe = xml_escape(&exe.to_string_lossy()),
            arg0 = SYNC_ARGS[0],
            arg1 = SYNC_ARGS[1],
            calendar_intervals = launchd_calendar_intervals(),
            stdout = xml_escape(&stdout.to_string_lossy()),
            stderr = xml_escape(&stderr.to_string_lossy())
        ))
    }

    fn launchd_calendar_intervals() -> String {
        CALENDAR_HOURS
            .iter()
            .map(|hour| {
                format!(
                    "    <dict>\n      <key>Hour</key>\n      <integer>{hour}</integer>\n      <key>Minute</key>\n      <integer>0</integer>\n    </dict>\n"
                )
            })
            .collect()
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::io::Write;
    use std::process::Stdio;

    const SERVICE_NAME: &str = "tokenboard-sync.service";
    const TIMER_NAME: &str = "tokenboard-sync.timer";
    const CRON_BEGIN: &str = "# tokenboard autosync begin";
    const CRON_END: &str = "# tokenboard autosync end";

    pub fn install() -> Result<String> {
        ensure_tokenboard_dir()?;
        if systemd_user_available() {
            install_systemd()
        } else {
            install_cron()
        }
    }

    pub fn uninstall() -> Result<String> {
        let mut removed = Vec::new();

        if systemd_user_dir()?.join(TIMER_NAME).exists()
            || systemd_user_dir()?.join(SERVICE_NAME).exists()
        {
            let _ = Command::new("systemctl")
                .args(["--user", "disable", "--now", TIMER_NAME])
                .status();
            let timer_path = systemd_user_dir()?.join(TIMER_NAME);
            let service_path = systemd_user_dir()?.join(SERVICE_NAME);
            if timer_path.exists() {
                std::fs::remove_file(&timer_path)
                    .with_context(|| format!("Failed to remove {}", timer_path.display()))?;
            }
            if service_path.exists() {
                std::fs::remove_file(&service_path)
                    .with_context(|| format!("Failed to remove {}", service_path.display()))?;
            }
            let _ = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            removed.push("systemd user timer");
        }

        if remove_cron_entry()? {
            removed.push("cron entry");
        }

        if removed.is_empty() {
            Ok("Automatic sync was not installed.".to_string())
        } else {
            Ok(format!(
                "Automatic sync removed from {}.",
                removed.join(" and ")
            ))
        }
    }

    pub fn status() -> Result<String> {
        if systemd_user_dir()?.join(TIMER_NAME).exists() {
            let active =
                command_text(Command::new("systemctl").args(["--user", "is-active", TIMER_NAME]))
                    .unwrap_or_else(|_| "unknown".to_string());
            let enabled =
                command_text(Command::new("systemctl").args(["--user", "is-enabled", TIMER_NAME]))
                    .unwrap_or_else(|_| "unknown".to_string());
            return Ok(format!(
                "Automatic sync uses systemd user timer {}. Active: {}. Enabled: {}.",
                TIMER_NAME,
                active.trim(),
                enabled.trim()
            ));
        }

        if crontab_contains_autosync()? {
            return Ok(
                "Automatic sync is installed as a cron entry and runs every 3 hours.".to_string(),
            );
        }

        Ok("Automatic sync is not installed.".to_string())
    }

    fn install_systemd() -> Result<String> {
        let exe = current_exe()?;
        let dir = systemd_user_dir()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create {}", dir.display()))?;

        let service_path = dir.join(SERVICE_NAME);
        let timer_path = dir.join(TIMER_NAME);

        std::fs::write(&service_path, systemd_service(&exe))
            .with_context(|| format!("Failed to write {}", service_path.display()))?;
        std::fs::write(&timer_path, systemd_timer())
            .with_context(|| format!("Failed to write {}", timer_path.display()))?;

        run_command(
            Command::new("systemctl").args(["--user", "daemon-reload"]),
            "reload systemd user units",
        )?;
        run_command(
            Command::new("systemctl").args(["--user", "enable", "--now", TIMER_NAME]),
            "enable tokenboard systemd timer",
        )?;

        Ok(format!(
            "Automatic sync installed with systemd user timer {}. It runs every 3 hours.",
            TIMER_NAME
        ))
    }

    fn install_cron() -> Result<String> {
        let exe = current_exe()?;
        let existing = read_crontab()?;
        let mut updated = without_marked_block(&existing, CRON_BEGIN, CRON_END);
        updated.push_str(&cron_entry(&exe)?);
        write_crontab(&updated)?;

        Ok(format!(
            "Automatic sync installed as a cron entry. It runs every 3 hours. Logs: {}",
            log_path("autosync.log")?.display()
        ))
    }

    fn systemd_user_available() -> bool {
        Command::new("systemctl")
            .args(["--user", "show-environment"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn systemd_user_dir() -> Result<PathBuf> {
        Ok(home_dir()?.join(".config").join("systemd").join("user"))
    }

    fn systemd_service(exe: &Path) -> String {
        format!(
            "[Unit]\nDescription=Sync tokenboard usage\n\n[Service]\nType=oneshot\nExecStart={} {} {}\n",
            systemd_quote(exe),
            SYNC_ARGS[0],
            SYNC_ARGS[1]
        )
    }

    fn systemd_timer() -> String {
        format!(
            "[Unit]\nDescription=Run tokenboard sync every 3 hours\n\n[Timer]\nOnBootSec=2min\nOnUnitActiveSec={}h\nPersistent=true\nUnit={}\n\n[Install]\nWantedBy=timers.target\n",
            INTERVAL_HOURS, SERVICE_NAME
        )
    }

    fn systemd_quote(path: &Path) -> String {
        let value = path
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%");
        format!("\"{value}\"")
    }

    fn cron_entry(exe: &Path) -> Result<String> {
        let stdout = log_path("autosync.log")?;
        let stderr = log_path("autosync.err")?;
        Ok(format!(
            "{begin}\n0 */{hours} * * * {exe} {arg0} {arg1} >> {stdout} 2>> {stderr}\n{end}\n",
            begin = CRON_BEGIN,
            hours = INTERVAL_HOURS,
            exe = sh_quote(exe),
            arg0 = SYNC_ARGS[0],
            arg1 = SYNC_ARGS[1],
            stdout = sh_quote(&stdout),
            stderr = sh_quote(&stderr),
            end = CRON_END
        ))
    }

    fn read_crontab() -> Result<String> {
        let output = Command::new("crontab")
            .arg("-l")
            .output()
            .context("Failed to read existing crontab")?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Ok(String::new())
        }
    }

    fn write_crontab(content: &str) -> Result<()> {
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .spawn()
            .context("Failed to start crontab installer")?;
        let stdin = child
            .stdin
            .as_mut()
            .context("Failed to open crontab stdin")?;
        stdin
            .write_all(content.as_bytes())
            .context("Failed to write updated crontab")?;
        let status = child.wait().context("Failed to wait for crontab")?;
        if !status.success() {
            anyhow::bail!("Failed to install updated crontab");
        }
        Ok(())
    }

    fn remove_cron_entry() -> Result<bool> {
        let existing = read_crontab()?;
        let updated = without_marked_block(&existing, CRON_BEGIN, CRON_END);
        if existing == updated {
            return Ok(false);
        }
        write_crontab(&updated)?;
        Ok(true)
    }

    fn crontab_contains_autosync() -> Result<bool> {
        Ok(read_crontab()?.contains(CRON_BEGIN))
    }

    fn command_text(command: &mut Command) -> Result<String> {
        let output = command.output().context("Failed to run status command")?;
        if !output.status.success() {
            anyhow::bail!("status command failed");
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    const TASK_NAME: &str = "TokenboardAutoSync";

    pub fn install() -> Result<String> {
        let exe = current_exe()?;
        let task = format!("\"{}\" {} {}", exe.display(), SYNC_ARGS[0], SYNC_ARGS[1]);
        let modifier = INTERVAL_HOURS.to_string();

        run_command(
            Command::new("schtasks").args([
                "/Create", "/TN", TASK_NAME, "/TR", &task, "/SC", "HOURLY", "/MO", &modifier, "/F",
            ]),
            "create Windows scheduled task",
        )?;

        Ok(format!(
            "Automatic sync installed with Windows Task Scheduler task {}. It runs every 3 hours.",
            TASK_NAME
        ))
    }

    pub fn uninstall() -> Result<String> {
        let output = Command::new("schtasks")
            .args(["/Delete", "/TN", TASK_NAME, "/F"])
            .output()
            .context("Failed to run schtasks")?;
        if output.status.success() {
            Ok(format!(
                "Automatic sync removed from Windows Task Scheduler task {}.",
                TASK_NAME
            ))
        } else {
            Ok("Automatic sync was not installed.".to_string())
        }
    }

    pub fn status() -> Result<String> {
        let output = Command::new("schtasks")
            .args(["/Query", "/TN", TASK_NAME])
            .output()
            .context("Failed to query Windows scheduled task")?;
        if output.status.success() {
            Ok(format!(
                "Automatic sync is installed as Windows Task Scheduler task {}.",
                TASK_NAME
            ))
        } else {
            Ok("Automatic sync is not installed.".to_string())
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    pub fn install() -> Result<String> {
        anyhow::bail!("Automatic sync is only supported on Linux, macOS, and Windows")
    }

    pub fn uninstall() -> Result<String> {
        anyhow::bail!("Automatic sync is only supported on Linux, macOS, and Windows")
    }

    pub fn status() -> Result<String> {
        anyhow::bail!("Automatic sync is only supported on Linux, macOS, and Windows")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_existing_managed_block() {
        let existing = "MAILTO=a@example.com\n# tokenboard autosync begin\n0 */3 * * * old\n# tokenboard autosync end\n0 1 * * * backup\n";
        let cleaned = without_marked_block(
            existing,
            "# tokenboard autosync begin",
            "# tokenboard autosync end",
        );
        assert_eq!(cleaned, "MAILTO=a@example.com\n0 1 * * * backup\n");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        let quoted = sh_quote(Path::new("/tmp/tokenboard's/bin"));
        assert_eq!(quoted, "'/tmp/tokenboard'\"'\"'s/bin'");
    }

    #[test]
    fn xml_escape_handles_plist_special_chars() {
        assert_eq!(
            xml_escape("/tmp/a&b<c>\"d'e"),
            "/tmp/a&amp;b&lt;c&gt;&quot;d&apos;e"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_plist_uses_wall_clock_calendar_intervals() {
        let plist = platform::launchd_plist(Path::new("/tmp/tokenboard")).unwrap();

        assert!(!plist.contains("<key>StartInterval</key>"));
        assert!(plist.contains("<key>StartCalendarInterval</key>"));
        for hour in [0, 3, 6, 9, 12, 15, 18, 21] {
            assert!(plist.contains(&format!("<integer>{hour}</integer>")));
        }
        assert!(plist.contains("<key>RunAtLoad</key>"));
    }
}
