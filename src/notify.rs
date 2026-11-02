//! Desktop notification at the end of a run, best effort: a failure is never an error.

use std::process::{Command, Stdio};

/// Shows a desktop notification with the given title and message.
pub fn send(title: &str, message: &str) {
    let mut command = notification_command(title, message);
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn notification_command(title: &str, message: &str) -> Command {
    let script = format!(
        "display notification {} with title {}",
        applescript_string(message),
        applescript_string(title)
    );
    let mut command = Command::new("osascript");
    command.arg("-e").arg(script);
    command
}

#[cfg(target_os = "windows")]
fn notification_command(title: &str, message: &str) -> Command {
    // A balloon tip of a temporary tray icon: available on every Windows without modules.
    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; \
         $n = New-Object System.Windows.Forms.NotifyIcon; \
         $n.Icon = [System.Drawing.SystemIcons]::Information; \
         $n.Visible = $true; \
         $n.ShowBalloonTip(10000, {}, {}, 'Info'); \
         Start-Sleep -Seconds 10; $n.Dispose()",
        powershell_string(title),
        powershell_string(message)
    );
    let mut command = Command::new("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn notification_command(title: &str, message: &str) -> Command {
    let mut command = Command::new("notify-send");
    command.arg(title).arg(message);
    command
}

/// Quotes a string for AppleScript.
pub fn applescript_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Quotes a string for PowerShell (single quotes, doubled inside).
pub fn powershell_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_applescript_strings() {
        assert_eq!(applescript_string(r#"a "b" \c"#), r#""a \"b\" \\c""#);
    }

    #[test]
    fn quotes_powershell_strings() {
        assert_eq!(powershell_string("it's"), "'it''s'");
    }
}
