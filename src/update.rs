//! Self-update from GitHub releases (DSH-style: rolling nightly or stable).
//!
//! Two channels, selected in `~/.dst/config.yml` (`update.channel`, default
//! `nightly`):
//!   * `nightly` — the rolling `nightly` prerelease, replaced on every commit to
//!     `main`. "Newer" means the published binary differs from the running one,
//!     compared by SHA-256.
//!   * `stable` — the newest versioned release. "Newer" means its `vX.Y.Z` tag
//!     is greater than the running binary's version.
//!
//! No crates and no HTTP client dependency: metadata and the binary come from
//! `curl`, hashes from `sha256sum`/`shasum`, and the replacement is verified and
//! atomic (write `.dst.update.<pid>` next to the binary, then rename over it),
//! followed by re-exec so the new build is running immediately.
//!
//! The startup check runs on a background thread and only *offers* an update;
//! `/update` checks and installs on demand.

use std::path::Path;
use std::sync::Mutex;

/// Repository that publishes the release assets. Override base (tests/local
/// mirrors) with `DST_UPDATE_BASE`.
pub const DEFAULT_BASE: &str =
    "https://github.com/MarsTechHAN/deepseek-harness-tui-cardputer";

pub const USER_AGENT: &str = "dst-updater";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Channel {
    Nightly,
    Stable,
}

impl Channel {
    pub fn parse(text: &str) -> Option<Channel> {
        match text.trim().to_ascii_lowercase().as_str() {
            "nightly" => Some(Channel::Nightly),
            "stable" => Some(Channel::Stable),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Nightly => "nightly",
            Channel::Stable => "stable",
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub channel: Channel,
    /// Version tag for stable (`v0.2.0`), or `nightly` for the rolling build.
    pub version: String,
    pub asset: String,
    pub url: String,
    pub sha256: String,
}

fn base() -> String {
    std::env::var("DST_UPDATE_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Release asset for this platform (None: self-update is unsupported here).
pub fn asset_name() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Some("dst-linux-aarch64"),
        "x86_64" => Some("dst-linux-x86_64"),
        _ => None,
    }
}

fn curl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "30", "-A", USER_AGENT])
        .args(args)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl failed: {}", err.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn final_url(url: &str) -> Result<String, String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            "--max-time",
            "30",
            "-A",
            USER_AGENT,
        ])
        .arg(url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("curl failed: {}", err.trim()));
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return Err("no redirect target from GitHub".into());
    }
    Ok(text)
}

/// `v0.2.0` / `0.2.0` -> the numeric version string.
pub fn tag_version(tag: &str) -> Option<String> {
    let v = tag.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    if v.is_empty() {
        return None;
    }
    let ok = v
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Some(v.to_string())
    } else {
        None
    }
}

fn version_parts(v: &str) -> Vec<u64> {
    v.split('.')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect()
}

/// Is `a` strictly newer than `b`? Numeric dotted comparison; missing parts are
/// padded with zero (`0.2` == `0.2.0`).
pub fn version_newer(a: &str, b: &str) -> bool {
    let mut av = version_parts(a);
    let mut bv = version_parts(b);
    let n = av.len().max(bv.len());
    av.resize(n, 0);
    bv.resize(n, 0);
    av > bv
}

/// `SHA256SUMS` text -> map of file name -> hex digest.
pub fn parse_sha256sums(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(sha) = fields.next() else { continue };
        let Some(name) = fields.next() else { continue };
        let name = name.trim_start_matches('*');
        if sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
            out.insert(name.to_string(), sha.to_ascii_lowercase());
        }
    }
    out
}

/// SHA-256 of a file, via coreutils (`sha256sum`) or BSD (`shasum -a 256`).
pub fn file_sha256(path: &Path) -> Option<String> {
    if let Ok(out) = std::process::Command::new("sha256sum").arg(path).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(sha) = text.split_whitespace().next() {
                let sha = sha.to_ascii_lowercase();
                if sha.len() == 64 {
                    return Some(sha);
                }
            }
        }
    }
    let out = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let sha = text.split_whitespace().next()?.to_ascii_lowercase();
    if sha.len() == 64 {
        Some(sha)
    } else {
        None
    }
}

fn our_sha256() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate the running binary: {e}"))?;
    file_sha256(&exe).ok_or_else(|| "cannot hash the running binary (sha256sum/shasum missing)".to_string())
}

/// Check the selected channel. `Ok(None)` means the running build is current.
pub fn check(channel: Channel) -> Result<Option<UpdateInfo>, String> {
    let asset = asset_name().ok_or_else(|| {
        format!(
            "self-update is not available for {} (only linux aarch64/x86_64 builds are published)",
            std::env::consts::ARCH
        )
    })?;
    let b = base();
    match channel {
        Channel::Stable => {
            let effective = final_url(&format!("{b}/releases/latest"))?;
            let tag = effective
                .rsplit("/tag/")
                .next()
                .filter(|_| effective.contains("/tag/"))
                .unwrap_or("")
                .to_string();
            if tag.is_empty() {
                return Err(format!("could not resolve the latest release tag ({effective})"));
            }
            let version = tag_version(&tag)
                .ok_or_else(|| format!("unexpected release tag {tag:?}"))?;
            if !version_newer(&version, env!("CARGO_PKG_VERSION")) {
                return Ok(None);
            }
            let sums = curl(&[&format!("{b}/releases/latest/download/SHA256SUMS")])?;
            let sha256 = parse_sha256sums(&sums)
                .get(asset)
                .cloned()
                .ok_or_else(|| format!("SHA256SUMS of {tag} has no {asset}"))?;
            Ok(Some(UpdateInfo {
                channel,
                version,
                asset: asset.to_string(),
                url: format!("{b}/releases/latest/download/{asset}"),
                sha256,
            }))
        }
        Channel::Nightly => {
            let sums = curl(&[&format!("{b}/releases/download/nightly/SHA256SUMS")])?;
            let sha256 = parse_sha256sums(&sums)
                .get(asset)
                .cloned()
                .ok_or_else(|| format!("nightly SHA256SUMS has no {asset}"))?;
            if our_sha256()?.eq_ignore_ascii_case(&sha256) {
                return Ok(None);
            }
            Ok(Some(UpdateInfo {
                channel,
                version: "nightly".into(),
                asset: asset.to_string(),
                url: format!("{b}/releases/download/nightly/{asset}"),
                sha256,
            }))
        }
    }
}

/// Download, verify and atomically replace the running binary.
pub fn install(info: &UpdateInfo, mut status: impl FnMut(&str)) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate the running binary: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", exe.display()))?;
    let tmp = dir.join(format!(".dst.update.{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);

    status("downloading");
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "300", "-A", USER_AGENT, "-o"])
        .arg(&tmp)
        .arg(&info.url)
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("download failed: {}", err.trim()));
    }

    status("verifying");
    let got = file_sha256(&tmp).ok_or_else(|| "cannot hash the downloaded binary".to_string())?;
    if !got.eq_ignore_ascii_case(&info.sha256) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "checksum mismatch: expected {}, got {got}",
            info.sha256
        ));
    }

    status("installing");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    // Rename over the running binary in the binary's own directory (same
    // filesystem, atomic); a running Linux executable may be replaced this way.
    std::fs::rename(&tmp, &exe).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!(
            "cannot replace {}: {e} — run dst as root or update with tools/deploy/deploy.sh",
            exe.display()
        )
    })?;
    Ok(())
}

/// Replace this process with the freshly installed binary (same argv).
pub fn restart_now() -> ! {
    crate::stty_tty(&["sane", "min", "1", "time", "0"]);
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("dst"));
    let err = std::process::Command::new(&exe)
        .args(std::env::args_os().skip(1))
        .exec();
    eprintln!("dst: updated, but could not restart {}: {err}", exe.display());
    std::process::exit(1);
}

/// Should self-update run in this process? Debug builds (developer checkouts)
/// do not know which release they correspond to; `DST_UPDATE_FORCE=1` enables
/// the flow for tests.
pub fn enabled() -> bool {
    if std::env::var("DST_NO_UPDATE_CHECK").map(|v| v == "1").unwrap_or(false) {
        return false;
    }
    !cfg!(debug_assertions)
        || std::env::var("DST_UPDATE_FORCE")
            .map(|v| v == "1")
            .unwrap_or(false)
}

static PENDING: Mutex<Option<Result<Option<UpdateInfo>, String>>> = Mutex::new(None);

/// Background startup check: one thread, result collected by the TUI loop.
pub fn spawn_check(channel: Channel) {
    if !enabled() {
        return;
    }
    if let Ok(mut slot) = PENDING.lock() {
        if slot.is_some() {
            return; // a check is already in flight / finished
        }
        *slot = None;
    }
    std::thread::spawn(move || {
        let result = check(channel);
        if let Ok(mut slot) = PENDING.lock() {
            *slot = Some(result);
        }
    });
}

/// Take a finished background result, if any (`None` = nothing to report yet).
pub fn take_check() -> Option<Result<Option<UpdateInfo>, String>> {
    PENDING.lock().ok().and_then(|mut slot| slot.take())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_parse_and_default_to_nightly() {
        assert_eq!(Channel::parse("nightly"), Some(Channel::Nightly));
        assert_eq!(Channel::parse(" Stable "), Some(Channel::Stable));
        assert_eq!(Channel::parse("latest"), None);
        assert_eq!(Channel::Nightly.as_str(), "nightly");
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_newer("0.2.0", "0.1.9"));
        assert!(version_newer("1.0.0", "0.9.9"));
        assert!(version_newer("0.1.10", "0.1.9"));
        assert!(!version_newer("0.1.0", "0.1.0"));
        assert!(!version_newer("0.1", "0.1.0"));
        assert!(version_newer("0.1.1", "0.1"));
    }

    #[test]
    fn release_tags_are_normalized() {
        assert_eq!(tag_version("v0.2.1"), Some("0.2.1".into()));
        assert_eq!(tag_version("0.2"), Some("0.2".into()));
        assert_eq!(tag_version("nightly"), None);
        assert_eq!(tag_version("v"), None);
        assert_eq!(tag_version("v1.2.3-rc1"), None);
    }

    #[test]
    fn sha256sums_parse_the_github_layout() {
        let sums = "b10fcf7d281a5e3611a70f47e6d8a2d372214190ca29aefa47eb0dcd982ba90c  dst-linux-aarch64\n\
                    e8798c0b26e01b8be5b56409f20a27096fb39d5b9a6be614cc81234f6d962d94 *dst-linux-x86_64\n\
                    garbage line\n";
        let map = parse_sha256sums(sums);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get("dst-linux-aarch64").map(|s| s.as_str()),
            Some("b10fcf7d281a5e3611a70f47e6d8a2d372214190ca29aefa47eb0dcd982ba90c")
        );
        assert!(map.contains_key("dst-linux-x86_64"));
    }

    #[test]
    fn asset_names_cover_the_published_platforms() {
        if std::env::consts::ARCH == "aarch64" {
            assert_eq!(asset_name(), Some("dst-linux-aarch64"));
        } else if std::env::consts::ARCH == "x86_64" {
            assert_eq!(asset_name(), Some("dst-linux-x86_64"));
        }
    }

    #[test]
    fn no_update_means_the_running_build_is_current() {
        // Current tag == running version on the stable channel: no offer.
        assert!(!version_newer(env!("CARGO_PKG_VERSION"), env!("CARGO_PKG_VERSION")));
    }
}
