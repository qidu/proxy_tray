//! Windows Defender inbound rule for the bundled proxy (doc §6).
//!
//! A tray-launched proxy is reachable from other hosts only if Windows Defender
//! allows inbound traffic to it, so a start ensures an inbound rule exists for
//! the sidecar. Adding a rule needs Administrator and the tray is not elevated,
//! so this is best-effort: the caller logs the outcome and starts the proxy
//! regardless.

/// Stable rule name, so a repeat start adds no duplicate.
#[cfg(windows)]
const RULE_NAME: &str = "Model Proxy Inbound";

/// The bundled sidecar sits next to the tray executable, and `externalBin`
/// strips the target triple, so it is `model-proxy-v3.exe` here.
#[cfg(windows)]
fn sidecar_path() -> Result<std::path::PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|err| format!("could not locate the tray executable: {err}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    Ok(dir.join("model-proxy-v3.exe"))
}

/// Ensure an inbound allow rule exists for the sidecar.
///
/// `Ok(Some(message))` describes what happened, `Ok(None)` means the platform
/// has no Windows Defender and there is nothing to report, and `Err` is a
/// failure the caller must surface (typically "run the tray as Administrator").
#[cfg(windows)]
pub fn ensure_inbound_rule() -> Result<Option<String>, String> {
    use windows_firewall::{add_rule_if_not_exists, Action, Direction, FirewallRule, Profile};

    let exe = sidecar_path()?;
    if !exe.exists() {
        return Err(format!("no sidecar to allow at {}", exe.display()));
    }

    let rule = FirewallRule::builder()
        .name(RULE_NAME)
        .action(Action::Allow)
        .direction(Direction::In)
        .enabled(true)
        .description("Allow model_proxy_v3 inbound traffic")
        // All profiles, explicitly: the proxy must be reachable on Domain,
        // Private and Public alike, and an unset profile defers to the COM
        // default rather than stating the intent.
        .profiles(Profile::All)
        .application_name(exe.to_string_lossy().to_string())
        .build();

    match add_rule_if_not_exists(&rule) {
        Ok(true) => Ok(Some(format!(
            "added inbound firewall rule {RULE_NAME:?} for {}",
            exe.display()
        ))),
        Ok(false) => Ok(Some(format!(
            "inbound firewall rule {RULE_NAME:?} already present"
        ))),
        Err(err) => Err(format!(
            "could not add inbound firewall rule {RULE_NAME:?} (run the tray as \
             Administrator?): {err}"
        )),
    }
}

/// No Windows Defender off Windows; nothing to do.
#[cfg(not(windows))]
pub fn ensure_inbound_rule() -> Result<Option<String>, String> {
    Ok(None)
}
