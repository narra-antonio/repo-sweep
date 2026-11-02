//! Registration of a periodic `sync` in the scheduler of the operating system: launchd on
//! macOS, Task Scheduler on Windows, a systemd user timer on Linux. No process stays in the
//! background: the operating system starts `repo-sweep` at the chosen time.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Identifier of the scheduled task (launchd label, Task Scheduler name, systemd unit).
pub const TASK_NAME: &str = "repo-sweep";
const LAUNCHD_LABEL: &str = "io.github.repo-sweep.sync";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weekday {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

impl Weekday {
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim().to_ascii_lowercase();
        let day = match value.get(..3)? {
            "mon" => Weekday::Mon,
            "tue" => Weekday::Tue,
            "wed" => Weekday::Wed,
            "thu" => Weekday::Thu,
            "fri" => Weekday::Fri,
            "sat" => Weekday::Sat,
            "sun" => Weekday::Sun,
            _ => return None,
        };
        Some(day)
    }

    /// Three-letter English name, as systemd and Task Scheduler expect it.
    pub fn short_name(self) -> &'static str {
        match self {
            Weekday::Mon => "Mon",
            Weekday::Tue => "Tue",
            Weekday::Wed => "Wed",
            Weekday::Thu => "Thu",
            Weekday::Fri => "Fri",
            Weekday::Sat => "Sat",
            Weekday::Sun => "Sun",
        }
    }

    /// launchd numbering: Sunday is 0.
    fn launchd_number(self) -> u8 {
        match self {
            Weekday::Sun => 0,
            Weekday::Mon => 1,
            Weekday::Tue => 2,
            Weekday::Wed => 3,
            Weekday::Thu => 4,
            Weekday::Fri => 5,
            Weekday::Sat => 6,
        }
    }
}

/// When the task runs: every day, or on one day of the week, at the given local time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct When {
    pub day: Option<Weekday>,
    pub hour: u8,
    pub minute: u8,
}

impl When {
    /// Parses a `HH:MM` time and an optional weekday.
    pub fn parse(at: &str, day: Option<&str>) -> Result<Self> {
        let (hour, minute) = at
            .trim()
            .split_once(':')
            .and_then(|(h, m)| Some((h.parse::<u8>().ok()?, m.parse::<u8>().ok()?)))
            .filter(|(h, m)| *h < 24 && *m < 60)
            .with_context(|| format!("invalid time '{at}': use HH:MM, for example 02:30"))?;
        let day = match day {
            Some(day) => Some(
                Weekday::parse(day)
                    .with_context(|| format!("invalid day '{day}': use mon, tue, ... sun"))?,
            ),
            None => None,
        };
        Ok(Self { day, hour, minute })
    }
}

impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.day {
            Some(day) => write!(
                f,
                "every {} at {:02}:{:02}",
                day.short_name(),
                self.hour,
                self.minute
            ),
            None => write!(f, "every day at {:02}:{:02}", self.hour, self.minute),
        }
    }
}

/// What to register.
#[derive(Debug, Clone)]
pub struct Task {
    pub when: When,
    /// Full command line: the executable followed by its arguments.
    pub command: Vec<String>,
    /// `PATH` of the installing shell, so that the task finds `git` and `ssh` as the user does.
    pub path_env: Option<String>,
    /// Folder of the task's own output (stdout/stderr).
    pub log_dir: PathBuf,
}

/// launchd property list (macOS).
pub fn launchd_plist(task: &Task) -> String {
    let arguments: String = task
        .command
        .iter()
        .map(|arg| format!("        <string>{}</string>\n", xml_escape(arg)))
        .collect();
    let mut interval = format!(
        "        <key>Hour</key>\n        <integer>{}</integer>\n        <key>Minute</key>\n        <integer>{}</integer>\n",
        task.when.hour, task.when.minute
    );
    if let Some(day) = task.when.day {
        interval.push_str(&format!(
            "        <key>Weekday</key>\n        <integer>{}</integer>\n",
            day.launchd_number()
        ));
    }
    let environment = task
        .path_env
        .as_ref()
        .map(|path| {
            format!(
                "    <key>EnvironmentVariables</key>\n    <dict>\n        <key>PATH</key>\n        <string>{}</string>\n    </dict>\n",
                xml_escape(path)
            )
        })
        .unwrap_or_default();
    let log = |name: &str| xml_escape(&task.log_dir.join(name).display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LAUNCHD_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{arguments}    </array>
    <key>StartCalendarInterval</key>
    <dict>
{interval}    </dict>
{environment}    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
</dict>
</plist>
"#,
        log("last-run.log"),
        log("last-run.err.log"),
    )
}

/// systemd user service and timer (Linux).
pub fn systemd_units(task: &Task) -> (String, String) {
    let exec: Vec<String> = task.command.iter().map(|arg| systemd_quote(arg)).collect();
    let environment = task
        .path_env
        .as_ref()
        .map(|path| format!("Environment={}\n", systemd_quote(&format!("PATH={path}"))))
        .unwrap_or_default();
    let service = format!(
        "[Unit]\nDescription=repo-sweep: sync the git repositories\n\n[Service]\nType=oneshot\n{environment}ExecStart={}\n",
        exec.join(" ")
    );
    let calendar = match task.when.day {
        Some(day) => format!(
            "{} *-*-* {:02}:{:02}:00",
            day.short_name(),
            task.when.hour,
            task.when.minute
        ),
        None => format!("*-*-* {:02}:{:02}:00", task.when.hour, task.when.minute),
    };
    let timer = format!(
        "[Unit]\nDescription=repo-sweep: scheduled sync\n\n[Timer]\nOnCalendar={calendar}\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
    );
    (service, timer)
}

/// Batch file run by Task Scheduler (Windows): it keeps the task command short (Task Scheduler
/// limits it to 261 characters) and redirects the output to the log folder.
pub fn windows_script(task: &Task) -> String {
    let command: Vec<String> = task.command.iter().map(|arg| cmd_quote(arg)).collect();
    let log = cmd_quote(&task.log_dir.join("last-run.log").display().to_string());
    format!("@echo off\r\n{} > {log} 2>&1\r\n", command.join(" "))
}

/// Arguments of `schtasks /Create` for the batch file.
pub fn schtasks_create_args(when: &When, script: &Path) -> Vec<String> {
    let mut args = vec![
        "/Create".to_string(),
        "/F".to_string(),
        "/TN".to_string(),
        TASK_NAME.to_string(),
        "/TR".to_string(),
        format!("\"{}\"", script.display()),
        "/ST".to_string(),
        format!("{:02}:{:02}", when.hour, when.minute),
    ];
    match when.day {
        Some(day) => args.extend([
            "/SC".to_string(),
            "WEEKLY".to_string(),
            "/D".to_string(),
            day.short_name().to_ascii_uppercase(),
        ]),
        None => args.extend(["/SC".to_string(), "DAILY".to_string()]),
    }
    args
}

/// Registers the task, replacing a previous one. Returns a description of what was done.
pub fn install(task: &Task) -> Result<String> {
    fs::create_dir_all(&task.log_dir)
        .with_context(|| format!("cannot create {}", task.log_dir.display()))?;
    platform::install(task)
}

/// Removes the task. Returns a description of what was done.
pub fn remove() -> Result<String> {
    platform::remove()
}

/// Describes the registered task.
pub fn status() -> Result<String> {
    platform::status()
}

fn run(program: &str, args: &[&str]) -> Result<std::process::Output> {
    Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {program}"))
}

fn run_ok(program: &str, args: &[&str]) -> Result<String> {
    let output = run(program, args)?;
    if !output.status.success() {
        bail!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn plist_path() -> Result<PathBuf> {
        Ok(dirs::home_dir()
            .context("cannot find the home folder")?
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")))
    }

    fn domain() -> Result<String> {
        Ok(format!("gui/{}", run_ok("id", &["-u"])?))
    }

    pub fn install(task: &Task) -> Result<String> {
        let path = plist_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let domain = domain()?;
        // Unload a previous version; it is fine if there is none.
        let _ = run(
            "launchctl",
            &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
        );
        fs::write(&path, launchd_plist(task))
            .with_context(|| format!("cannot write {}", path.display()))?;
        run_ok(
            "launchctl",
            &["bootstrap", &domain, &path.display().to_string()],
        )?;
        Ok(format!(
            "launchd agent {LAUNCHD_LABEL} ({})",
            path.display()
        ))
    }

    pub fn remove() -> Result<String> {
        let path = plist_path()?;
        let _ = run(
            "launchctl",
            &["bootout", &format!("{}/{LAUNCHD_LABEL}", domain()?)],
        );
        if path.exists() {
            fs::remove_file(&path)?;
            Ok(format!("removed {}", path.display()))
        } else {
            Ok("no scheduled task".to_string())
        }
    }

    pub fn status() -> Result<String> {
        let path = plist_path()?;
        if !path.exists() {
            return Ok("no scheduled task".to_string());
        }
        let loaded = run(
            "launchctl",
            &["print", &format!("{}/{LAUNCHD_LABEL}", domain()?)],
        )?
        .status
        .success();
        Ok(format!(
            "{} ({})\n{}",
            path.display(),
            if loaded { "loaded" } else { "not loaded" },
            fs::read_to_string(&path)?
        ))
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    fn script_path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("cannot find the configuration folder")?
            .join("repo-sweep")
            .join("scheduled-sync.cmd"))
    }

    pub fn install(task: &Task) -> Result<String> {
        let script = script_path()?;
        if let Some(parent) = script.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&script, windows_script(task))
            .with_context(|| format!("cannot write {}", script.display()))?;
        let args = schtasks_create_args(&task.when, &script);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run_ok("schtasks", &args)?;
        Ok(format!(
            "Task Scheduler task {TASK_NAME} ({})",
            script.display()
        ))
    }

    pub fn remove() -> Result<String> {
        let output = run("schtasks", &["/Delete", "/F", "/TN", TASK_NAME])?;
        let script = script_path()?;
        if script.exists() {
            fs::remove_file(&script)?;
        }
        Ok(if output.status.success() {
            format!("removed task {TASK_NAME}")
        } else {
            "no scheduled task".to_string()
        })
    }

    pub fn status() -> Result<String> {
        let output = run(
            "schtasks",
            &["/Query", "/TN", TASK_NAME, "/V", "/FO", "LIST"],
        )?;
        Ok(if output.status.success() {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        } else {
            "no scheduled task".to_string()
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod platform {
    use super::*;

    fn unit_dir() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("cannot find the configuration folder")?
            .join("systemd/user"))
    }

    pub fn install(task: &Task) -> Result<String> {
        let dir = unit_dir()?;
        fs::create_dir_all(&dir)?;
        let (service, timer) = systemd_units(task);
        fs::write(dir.join(format!("{TASK_NAME}.service")), service)?;
        fs::write(dir.join(format!("{TASK_NAME}.timer")), timer)?;
        run_ok("systemctl", &["--user", "daemon-reload"])?;
        run_ok(
            "systemctl",
            &["--user", "enable", "--now", &format!("{TASK_NAME}.timer")],
        )?;
        Ok(format!(
            "systemd user timer {TASK_NAME}.timer ({})",
            dir.display()
        ))
    }

    pub fn remove() -> Result<String> {
        let dir = unit_dir()?;
        let timer = dir.join(format!("{TASK_NAME}.timer"));
        if !timer.exists() {
            return Ok("no scheduled task".to_string());
        }
        let _ = run(
            "systemctl",
            &["--user", "disable", "--now", &format!("{TASK_NAME}.timer")],
        );
        fs::remove_file(&timer)?;
        let service = dir.join(format!("{TASK_NAME}.service"));
        if service.exists() {
            fs::remove_file(service)?;
        }
        let _ = run("systemctl", &["--user", "daemon-reload"]);
        Ok(format!("removed {TASK_NAME}.timer"))
    }

    pub fn status() -> Result<String> {
        let timer = unit_dir()?.join(format!("{TASK_NAME}.timer"));
        if !timer.exists() {
            return Ok("no scheduled task".to_string());
        }
        let output = run(
            "systemctl",
            &["--user", "list-timers", &format!("{TASK_NAME}.timer")],
        )?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Quotes an argument for a systemd `ExecStart=` line.
fn systemd_quote(arg: &str) -> String {
    let escaped = arg
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    if escaped.is_empty() || escaped.contains(char::is_whitespace) || escaped != arg {
        format!("\"{escaped}\"")
    } else {
        escaped
    }
}

/// Quotes an argument for a Windows batch file.
fn cmd_quote(arg: &str) -> String {
    format!("\"{}\"", arg.replace('%', "%%").replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(day: Option<Weekday>) -> Task {
        Task {
            when: When {
                day,
                hour: 2,
                minute: 5,
            },
            command: vec![
                "/opt/repo sweep/repo-sweep".to_string(),
                "sync".to_string(),
                "/work & play".to_string(),
            ],
            path_env: Some("/usr/bin:/bin".to_string()),
            log_dir: PathBuf::from("/logs"),
        }
    }

    #[test]
    fn parses_when() {
        assert_eq!(
            When::parse("02:30", None).unwrap(),
            When {
                day: None,
                hour: 2,
                minute: 30
            }
        );
        assert_eq!(
            When::parse("8:00", Some("Monday")).unwrap().day,
            Some(Weekday::Mon)
        );
        assert!(When::parse("24:00", None).is_err());
        assert!(When::parse("02-30", None).is_err());
        assert!(When::parse("02:30", Some("funday")).is_err());
    }

    #[test]
    fn describes_when() {
        assert_eq!(
            When::parse("02:05", None).unwrap().to_string(),
            "every day at 02:05"
        );
        assert_eq!(
            When::parse("08:00", Some("sun")).unwrap().to_string(),
            "every Sun at 08:00"
        );
    }

    #[test]
    fn builds_launchd_plist() {
        let plist = launchd_plist(&task(Some(Weekday::Sun)));
        assert!(plist.contains("<string>/opt/repo sweep/repo-sweep</string>"));
        assert!(plist.contains("<string>/work &amp; play</string>"));
        assert!(plist.contains("<key>Hour</key>\n        <integer>2</integer>"));
        assert!(plist.contains("<key>Minute</key>\n        <integer>5</integer>"));
        assert!(plist.contains("<key>Weekday</key>\n        <integer>0</integer>"));
        assert!(plist.contains("<string>/usr/bin:/bin</string>"));
        assert!(!launchd_plist(&task(None)).contains("Weekday"));
    }

    #[test]
    fn builds_systemd_units() {
        let (service, timer) = systemd_units(&task(Some(Weekday::Mon)));
        assert!(service.contains("ExecStart=\"/opt/repo sweep/repo-sweep\" sync \"/work & play\""));
        assert!(service.contains("Environment=PATH=/usr/bin:/bin"));
        assert!(timer.contains("OnCalendar=Mon *-*-* 02:05:00"));
        assert!(timer.contains("Persistent=true"));
        let (_, daily) = systemd_units(&task(None));
        assert!(daily.contains("OnCalendar=*-*-* 02:05:00"));
    }

    #[test]
    fn quotes_systemd_arguments() {
        assert_eq!(systemd_quote("plain"), "plain");
        assert_eq!(systemd_quote("50%"), "\"50%%\"");
        assert_eq!(systemd_quote(""), "\"\"");
    }

    #[test]
    fn builds_windows_script_and_schtasks_args() {
        let script = windows_script(&task(None));
        assert!(script.starts_with("@echo off\r\n"));
        assert!(script.contains("\"/opt/repo sweep/repo-sweep\" \"sync\""));
        let args = schtasks_create_args(
            &When::parse("02:05", Some("fri")).unwrap(),
            Path::new("C:\\rs\\run.cmd"),
        );
        assert!(args.windows(2).any(|w| w == ["/SC", "WEEKLY"]));
        assert!(args.windows(2).any(|w| w == ["/D", "FRI"]));
        assert!(args.windows(2).any(|w| w == ["/ST", "02:05"]));
        assert!(args.windows(2).any(|w| w == ["/TR", "\"C:\\rs\\run.cmd\""]));
    }
}
