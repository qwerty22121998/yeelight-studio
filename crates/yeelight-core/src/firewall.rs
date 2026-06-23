//! Open static inbound ports via `ufw` (Linux).
//!
//! Security-sensitive: these helpers modify the system firewall and `ufw` requires
//! root. They never run silently — every action is logged at INFO. If `ufw` is
//! missing or inactive they no-op; if it fails (typically lack of root) they return
//! [`Error::Firewall`] with the exact command to run manually.
//!
//! The plain helpers ([`ensure_udp_open`], [`ensure_tcp_open`]) never escalate. The
//! `*_pkexec` variants opt in to running `pkexec ufw allow`, which asks for a password
//! through the desktop's polkit agent (a graphical dialog) — so they work from a GUI
//! with no controlling terminal, unlike a bare `sudo`.

use tokio::process::Command;

use crate::error::{Error, Result};

/// Ensure inbound UDP `port` is allowed (discovery uses 1982).
pub async fn ensure_udp_open(port: u16) -> Result<()> {
    ensure_open(port, "udp").await
}

/// Ensure inbound TCP `port` is allowed (music mode).
pub async fn ensure_tcp_open(port: u16) -> Result<()> {
    ensure_open(port, "tcp").await
}

/// Whether inbound UDP `port` is already reachable — i.e. nothing is blocking it.
///
/// `true` when `ufw` is missing, inactive, or already allows the rule; `false`
/// when `ufw` is active without a matching rule, or its status can't be read
/// (typically needs root). Use this to decide whether discovery can run before
/// opening the firewall, rather than opening it unconditionally.
pub async fn is_udp_open(port: u16) -> Result<bool> {
    is_open(port, "udp").await
}

/// Like [`ensure_udp_open`] but escalates via `pkexec` (graphical polkit prompt).
pub async fn ensure_udp_open_pkexec(port: u16) -> Result<()> {
    pkexec_allow(&format!("{port}/udp")).await
}

/// Like [`ensure_tcp_open`] but escalates via `pkexec` (graphical polkit prompt).
pub async fn ensure_tcp_open_pkexec(port: u16) -> Result<()> {
    pkexec_allow(&format!("{port}/tcp")).await
}

/// Run `pkexec ufw allow <rule>` so the desktop polkit agent prompts for the
/// password graphically — no controlling terminal required.
///
/// `ufw allow` is idempotent (re-adding an existing rule is a no-op), so no status
/// pre-check is needed. A dismissed or denied prompt surfaces as [`Error::Firewall`]
/// carrying the manual command.
async fn pkexec_allow(rule: &str) -> Result<()> {
    tracing::info!("opening firewall: pkexec ufw allow {rule} (graphical password prompt)");
    let status = match Command::new("pkexec")
        .arg("ufw")
        .arg("allow")
        .arg(rule)
        .status()
        .await
    {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Firewall(format!(
                "`pkexec` not found. Run manually: sudo ufw allow {rule}"
            )));
        }
        Err(e) => {
            return Err(Error::Firewall(format!(
                "running `pkexec ufw allow {rule}`: {e}"
            )));
        }
    };
    if !status.success() {
        return Err(Error::Firewall(format!(
            "opening the firewall was cancelled or failed. Run manually: sudo ufw allow {rule}"
        )));
    }
    Ok(())
}

async fn ensure_open(port: u16, proto: &str) -> Result<()> {
    let rule = format!("{port}/{proto}");

    let status = match Command::new("ufw").arg("status").output().await {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("ufw not installed; skipping firewall setup for {rule}");
            return Ok(());
        }
        Err(e) => return Err(Error::Firewall(format!("running `ufw status`: {e}"))),
    };

    if !status.status.success() {
        return Err(Error::Firewall(format!(
            "`ufw status` failed (needs root?). Run manually: sudo ufw allow {rule}"
        )));
    }

    let stdout = String::from_utf8_lossy(&status.stdout);
    if stdout.contains("Status: inactive") {
        tracing::info!("ufw inactive; skipping firewall setup for {rule}");
        return Ok(());
    }
    if stdout.contains(&rule) {
        tracing::info!("ufw already allows {rule}");
        return Ok(());
    }

    tracing::info!("opening firewall: ufw allow {rule}");
    let out = Command::new("ufw")
        .arg("allow")
        .arg(&rule)
        .output()
        .await
        .map_err(|e| Error::Firewall(format!("running `ufw allow {rule}`: {e}")))?;
    if !out.status.success() {
        return Err(Error::Firewall(format!(
            "`ufw allow {rule}` failed (needs root?). Run manually: sudo ufw allow {rule}"
        )));
    }
    Ok(())
}

/// Query `ufw` for whether `port`/`proto` is already permitted. See [`is_udp_open`].
async fn is_open(port: u16, proto: &str) -> Result<bool> {
    let rule = format!("{port}/{proto}");
    match Command::new("ufw").arg("status").output().await {
        Ok(o) if o.status.success() => Ok(status_allows(&String::from_utf8_lossy(&o.stdout), &rule)),
        // `ufw status` failed (typically needs root) → can't confirm → treat as closed.
        Ok(_) => Ok(false),
        // No ufw installed → nothing is blocking the port.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(Error::Firewall(format!("running `ufw status`: {e}"))),
    }
}

/// `true` if a `ufw status` dump shows the firewall inactive or carrying `rule`.
fn status_allows(stdout: &str, rule: &str) -> bool {
    stdout.contains("Status: inactive") || stdout.contains(rule)
}

#[cfg(test)]
mod tests {
    use super::status_allows;

    #[test]
    fn reads_ufw_status() {
        let active = "Status: active\n\nTo  Action  From\n--  ------  ----\n1982/udp  ALLOW  Anywhere\n";
        assert!(status_allows(active, "1982/udp"));
        assert!(!status_allows(active, "55443/tcp"));
        assert!(status_allows("Status: inactive\n", "1982/udp"));
    }
}
