//! Open static inbound ports via `ufw` (Linux).
//!
//! Security-sensitive: these helpers modify the system firewall and `ufw` requires
//! root. They never run silently — every action is logged at INFO. If `ufw` is
//! missing or inactive they no-op; if it fails (typically lack of root) they return
//! [`Error::Firewall`] with the exact command to run manually.
//!
//! The plain helpers ([`ensure_udp_open`], [`ensure_tcp_open`]) never escalate. The
//! `*_sudo` variants opt in to running `sudo ufw allow`, which prompts for a password
//! on the controlling terminal — so they only work from an interactive shell (or with
//! a `NOPASSWD` sudoers rule / cached credentials), not from a non-TTY context.

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

/// Like [`ensure_udp_open`] but escalates via `sudo` (may prompt for a password).
pub async fn ensure_udp_open_sudo(port: u16) -> Result<()> {
    sudo_allow(&format!("{port}/udp")).await
}

/// Like [`ensure_tcp_open`] but escalates via `sudo` (may prompt for a password).
pub async fn ensure_tcp_open_sudo(port: u16) -> Result<()> {
    sudo_allow(&format!("{port}/tcp")).await
}

/// Run `sudo ufw allow <rule>` inheriting the terminal so the password prompt works.
///
/// `ufw allow` is idempotent (re-adding an existing rule is a no-op), so no status
/// pre-check is needed.
async fn sudo_allow(rule: &str) -> Result<()> {
    tracing::info!("opening firewall: sudo ufw allow {rule} (may prompt for password)");
    // `.status()` inherits stdin/stdout/stderr -> sudo can prompt on the TTY.
    let status = match Command::new("sudo")
        .arg("ufw")
        .arg("allow")
        .arg(rule)
        .status()
        .await
    {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::Firewall(format!(
                "`sudo` not found. Run manually: sudo ufw allow {rule}"
            )));
        }
        Err(e) => return Err(Error::Firewall(format!("running `sudo ufw allow {rule}`: {e}"))),
    };
    if !status.success() {
        return Err(Error::Firewall(format!(
            "`sudo ufw allow {rule}` failed. Run manually: sudo ufw allow {rule}"
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
