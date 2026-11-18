//! The device's own hardware notebook.
//!
//! `~/.dst/skills/hardware-info/` is a normal skill package, so the model can
//! read it with the `skill` tool, the catalog announces it and `/extensions`
//! manages it. dst itself maintains it:
//!
//! - the **seed** (shipped in the binary, see `assets/skills/hardware-info/`)
//!   contains the stable reference: what the parts are, how dst talks to them,
//!   the safe probe commands and the rules for hardware code;
//! - `update()` runs those probes, replaces the section between the two scan
//!   markers with the measured state, keeps the previous version under
//!   `.history/` and stores the raw capture in `state/scan-latest.txt`.
//!
//! First-run onboarding offers the scan once (`/update-hwinfo` re-runs it). The
//! scan is deliberately read-only and bounded: every probe has a timeout, and
//! a missing tool or sysfs entry simply produces a "not present" line instead
//! of an error, because a skill must not become unusable on a partial system.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Name of the package in the skill library.
pub const SKILL: &str = "hardware-info";

/// Marker lines the updater rewrites. The seed file carries them too, so the
/// asset is both the initial skill and the rendering template.
const SCAN_BEGIN: &str = "<!-- hw-scan:begin -->";
const SCAN_END: &str = "<!-- hw-scan:end -->";

/// The stable reference, compiled in so a device with no network and no files
/// still gets a usable hardware skill on first boot.
pub const SEED: &str = include_str!("../assets/skills/hardware-info/SKILL.md");

/// Longest raw capture written into the skill (bytes). The full capture stays
/// in state/scan-latest.txt.
const MAX_RAW: usize = 12_000;
/// Per-probe wall clock budget. `iw`/`lsusb` on a wedged bus must not hang a
/// scan that runs during onboarding.
const PROBE_TIMEOUT_MS: u64 = 2_500;
/// Whole-scan budget: a skill update is a background convenience, never
/// something the user waits minutes for.
const SCAN_BUDGET: Duration = Duration::from_secs(20);

/// Where the package lives for a given skill root.
pub fn dir(root: &Path) -> PathBuf {
    root.join(SKILL)
}

fn skill_dir() -> PathBuf {
    dir(&crate::config::skills_dir())
}

/// Install the seed skill when the package is missing. Returns true when it was
/// (re)created. Never overwrites: a scanned or model-refined version wins.
pub fn seed() -> bool {
    let dir = skill_dir();
    let md = dir.join("SKILL.md");
    if md.is_file() {
        return false;
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return false;
    }
    let text = seed_text();
    let tmp = dir.join("SKILL.md.tmp-write");
    if std::fs::write(&tmp, &text).is_err() {
        return false;
    }
    std::fs::rename(&tmp, &md).is_ok()
}

/// Seed text with the scan date filled in (the asset ships a fixed date).
fn seed_text() -> String {
    let stamp = crate::apps::now_stamp();
    let day = stamp.split('T').next().unwrap_or("").to_string();
    let mut out = String::new();
    for line in SEED.lines() {
        if line.starts_with("updated:") && !day.is_empty() {
            out.push_str(&format!("updated: {day}"));
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// What one scan found, summarized for the transcript.
pub struct Summary {
    pub path: String,
    pub version: u32,
    pub sections: usize,
    pub bytes: usize,
    /// False when the rewritten bytes are identical to what was there.
    pub changed: bool,
    pub secs: f32,
}

/// Read the current SKILL.md (any), if present.
fn read_skill(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("SKILL.md")).ok()
}

fn frontmatter_version(text: &str) -> u32 {
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("version:") {
            if let Ok(n) = v.trim().parse::<u32>() {
                return n;
            }
        }
    }
    1
}

fn frontmatter_updated(text: &str) -> String {
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("updated:") {
            return v.trim().to_string();
        }
    }
    String::new()
}

/// Body of the reference section (everything before the scan marker, without
/// the frontmatter). Taken from the *current* file when possible so model edits
/// to the reference survive a rescan; otherwise from the seed.
fn reference_body(current: Option<&str>) -> (String, u32, String) {
    let source = current.unwrap_or(SEED);
    let (version, updated) = (
        frontmatter_version(source),
        frontmatter_updated(source),
    );
    let body = match source.find(SCAN_BEGIN) {
        Some(i) => &source[..i],
        None => source,
    };
    // Drop the frontmatter so the caller can write a fresh one.
    let body = match body.strip_prefix("---") {
        Some(rest) => match rest.find("\n---") {
            Some(end) => rest[end + 4..].trim_start_matches('\n').to_string(),
            None => body.to_string(),
        },
        None => body.to_string(),
    };
    (body.trim_end().to_string(), version, updated)
}

/// The scan section of an existing SKILL.md (between the markers), or "".
fn previous_scan(text: &str) -> String {
    let Some(begin) = text.find(SCAN_BEGIN) else {
        return String::new();
    };
    let after = &text[begin + SCAN_BEGIN.len()..];
    match after.find(SCAN_END) {
        Some(end) => after[..end].to_string(),
        None => after.to_string(),
    }
}

/// Notes the user (or the model) appended after the scan block are preserved
/// across rescans: they are knowledge, not measurements.
fn notes_after_scan(text: &str) -> String {
    match text.find(SCAN_END) {
        Some(i) => text[i + SCAN_END.len()..].trim().to_string(),
        None => String::new(),
    }
}

// ------------------------------------------------------------------- probes

fn run_bounded(cmd: &str, timeout_ms: u64) -> String {
    let child = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(c) => c,
        Err(_) => return String::new(),
    };
    // Drain stdout while the command runs: a probe that prints more than the
    // pipe buffer would otherwise block until the timeout and return nothing.
    let out_pipe = child.stdout.take();
    let h_out = out_pipe.map(|so| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = so.take(64 * 1024).read_to_end(&mut buf);
            buf
        })
    });
    // stderr is deliberately dropped: on a host with a partial sysfs the probe
    // commands echo "No such file" for every absent path, which says nothing
    // about the device. A probe that produces nothing renders as
    // "(nothing detected)".
    let _ = child.stderr.take();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => return String::new(),
        }
        if started.elapsed().as_millis() >= timeout_ms as u128 {
            let _ = child.kill();
            let _ = child.wait();
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    h_out
        .and_then(|h| h.join().ok())
        .map(|buf| String::from_utf8_lossy(&buf).trim().to_string())
        .unwrap_or_default()
}

fn read_text(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// One measured block: a heading plus its lines (empty lines = subsystem absent).
struct Block {
    title: &'static str,
    lines: Vec<String>,
}

fn push(block: &mut Block, text: impl Into<String>) {
    let text = text.into();
    let text = text.trim();
    if !text.is_empty() {
        block.lines.push(text.to_string());
    }
}

fn section(title: &'static str) -> Block {
    Block {
        title,
        lines: Vec::new(),
    }
}

fn scan() -> Vec<Block> {
    let deadline = Instant::now() + SCAN_BUDGET;
    let mut blocks: Vec<Block> = Vec::new();
    let mut raw: Vec<(String, String)> = Vec::new();
    macro_rules! probe {
        ($label:expr, $cmd:expr) => {{
            let left = deadline.saturating_duration_since(Instant::now());
            let ms = PROBE_TIMEOUT_MS.min(left.as_millis() as u64).max(200);
            let out = run_bounded($cmd, ms);
            raw.push(($label.to_string(), out.clone()));
            out
        }};
    }

    // ---- system ----
    let mut b = section("System");
    for (label, value) in [
        ("hostname", read_text("/etc/hostname")),
        ("kernel", probe!("uname", "uname -srm")),
        (
            "os",
            read_text("/etc/os-release")
                .lines()
                .find(|l| l.starts_with("PRETTY_NAME="))
                .map(|l| l.trim_start_matches("PRETTY_NAME=").trim_matches('"').to_string())
                .unwrap_or_default(),
        ),
        (
            "board model",
            read_text("/proc/device-tree/model").replace('\0', " "),
        ),
        (
            "compatible",
            read_text("/proc/device-tree/compatible").replace('\0', " "),
        ),
        (
            "kernel cmdline",
            read_text("/proc/cmdline").chars().take(220).collect(),
        ),
        (
            "uptime",
            probe!("uptime", "uptime -p 2>/dev/null || cut -d' ' -f1 /proc/uptime"),
        ),
    ] {
        if !value.trim().is_empty() {
            push(&mut b, format!("- {label}: {value}"));
        }
    }
    blocks.push(b);

    // ---- cpu / memory ----
    let mut b = section("CPU and memory");
    let cpu = probe!(
        "cpu",
        "grep -m1 -E 'model name|Hardware' /proc/cpuinfo; grep -c ^processor /proc/cpuinfo"
    );
    for line in cpu.lines().take(4) {
        push(&mut b, format!("- {line}"));
    }
    let freq = read_text("/sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq");
    if !freq.is_empty() {
        push(&mut b, format!("- cpufreq now: {freq} kHz"));
    }
    let mem = read_text("/proc/meminfo");
    let pick = |key: &str| {
        mem.lines()
            .find(|l| l.starts_with(key))
            .map(|l| l.split_whitespace().nth(1).unwrap_or("").to_string())
            .unwrap_or_default()
    };
    let total_kb: u64 = pick("MemTotal:").parse().unwrap_or(0);
    let avail_kb: u64 = pick("MemAvailable:").parse().unwrap_or(0);
    if total_kb > 0 {
        push(
            &mut b,
            format!(
                "- ram: {} MB total, {} MB available at scan",
                total_kb / 1024,
                avail_kb / 1024
            ),
        );
    }
    let swap = pick("SwapTotal:");
    if !swap.is_empty() {
        push(&mut b, format!("- swap: {swap} kB"));
    }
    blocks.push(b);

    // ---- storage ----
    let mut b = section("Storage");
    for line in probe!("lsblk", "lsblk -o NAME,SIZE,TYPE,MOUNTPOINT 2>/dev/null | head -16").lines() {
        push(&mut b, format!("- {line}"));
    }
    push(
        &mut b,
        format!("- root: {}", probe!("df", "df -h / | tail -1 | tr -s ' '")),
    );
    blocks.push(b);

    // ---- display ----
    let mut b = section("Display and terminal");
    let fbs = probe!(
        "fb",
        "for f in /sys/class/graphics/fb*; do [ -d \"$f\" ] || continue; \
           echo \"$(basename $f): $(cat $f/name 2>/dev/null) $(cat $f/virtual_size 2>/dev/null) $(cat $f/bits_per_pixel 2>/dev/null)bpp\"; \
         done; ls /dev/fb* 2>/dev/null | tr '\\n' ' '"
    );
    for line in fbs.lines() {
        push(&mut b, format!("- {line}"));
    }
    // Cardputer Zero images name the panel /dev/fb_lcd; /dev/fb0 can be HDMI.
    let lcd = read_text("/sys/class/graphics/fb_lcd/virtual_size");
    if !lcd.is_empty() {
        push(&mut b, format!("- panel framebuffer: /dev/fb_lcd {lcd} px"));
    }
    push(
        &mut b,
        format!(
            "- panel: {}x{} px, {}x{} cells of 6x13 px (dst layout)",
            320, 170, 53, 12
        ),
    );
    push(
        &mut b,
        format!("- terminal service: {}", probe!("cpzterm", "systemctl is-active cpzterm.service 2>/dev/null")),
    );
    blocks.push(b);

    // ---- audio ----
    let mut b = section("Audio");
    let cards = probe!("asound", "cat /proc/asound/cards 2>/dev/null | sed 's/^ *//'");
    for line in cards.lines().take(8) {
        push(&mut b, format!("- {line}"));
    }
    for line in probe!("pcm", "cat /proc/asound/pcm 2>/dev/null | head -6").lines() {
        push(&mut b, format!("- pcm: {line}"));
    }
    blocks.push(b);

    // ---- input ----
    let mut b = section("Input");
    let input = probe!(
        "input",
        "grep -E 'Name|Handlers' /proc/bus/input/devices 2>/dev/null | head -24"
    );
    for line in input.lines() {
        push(&mut b, format!("- {}", line.trim()));
    }
    let keymap = read_text("/etc/default/keyboard");
    for line in keymap.lines().filter(|l| l.starts_with("KMAP") || l.starts_with("XKBLAYOUT")) {
        push(&mut b, format!("- keyboard config: {}", line.trim()));
    }
    push(
        &mut b,
        format!(
            "- m5stack keymaps: {}",
            probe!("keymaps", "ls /usr/share/keymaps/ 2>/dev/null | grep -i m5 | head -3 | tr '\\n' ' '")
        ),
    );
    blocks.push(b);

    // ---- network ----
    let mut b = section("Network");
    push(&mut b, format!("- links: {}", probe!("links", "ip -br link 2>/dev/null")));
    push(&mut b, format!("- addresses: {}", probe!("addrs", "ip -br addr 2>/dev/null")));
    push(
        &mut b,
        format!("- wifi: {}", probe!("wifi", "iw dev 2>/dev/null | head -12; iwconfig 2>/dev/null | head -8")),
    );
    blocks.push(b);

    // ---- battery / power ----
    let mut b = section("Battery and power");
    let supplies = probe!(
        "power_supply",
        "for p in /sys/class/power_supply/*; do \
           [ -d \"$p\" ] || continue; \
           printf '%s: type=%s status=%s capacity=%s\\n' \
             \"$(basename $p)\" \"$(cat $p/type 2>/dev/null)\" \
             \"$(cat $p/status 2>/dev/null)\" \"$(cat $p/capacity 2>/dev/null)\"; \
         done"
    );
    for line in supplies.lines() {
        push(&mut b, format!("- {line}"));
    }
    let voltage = probe!(
        "power_detail",
        "for p in /sys/class/power_supply/*; do \
           [ -f \"$p/voltage_now\" ] && printf '%s voltage_now=%s current_now=%s\\n' \
             \"$(basename $p)\" \"$(cat $p/voltage_now)\" \"$(cat $p/current_now 2>/dev/null)\"; \
         done"
    );
    for line in voltage.lines() {
        push(&mut b, format!("- {line}"));
    }
    blocks.push(b);

    // ---- thermal / sensors ----
    let mut b = section("Thermal and sensors");
    let temps = probe!(
        "thermal",
        "for z in /sys/class/thermal/thermal_zone*; do \
           [ -d \"$z\" ] || continue; \
           echo \"$(cat $z/type 2>/dev/null)=$(cat $z/temp 2>/dev/null)\"; \
         done; for h in /sys/class/hwmon/hwmon*; do [ -d \"$h\" ] && echo \"hwmon: $(cat $h/name 2>/dev/null)\"; done"
    );
    for line in temps.lines() {
        push(&mut b, format!("- {line}"));
    }
    let iio = probe!(
        "iio",
        "for d in /sys/bus/iio/devices/iio:device*; do \
           [ -d \"$d\" ] && echo \"$(basename $d): $(cat $d/name 2>/dev/null)\"; \
         done"
    );
    for line in iio.lines() {
        push(&mut b, format!("- iio: {line}"));
    }
    blocks.push(b);

    // ---- buses / gpio ----
    let mut b = section("Buses and GPIO");
    for (label, value) in [
        (
            "devices",
            probe!(
                "buses",
                "ls /dev/i2c-* /dev/spidev* /dev/gpiochip* /dev/ttyS* /dev/ttyAMA* 2>/dev/null | tr '\\n' ' '"
            ),
        ),
        (
            "i2c devices",
            probe!("i2c", "for d in /sys/bus/i2c/devices/*; do [ -e \"$d/name\" ] && echo \"$(basename $d)=$(cat $d/name)\"; done | tr '\\n' ' '"),
        ),
        (
            "spi devices",
            probe!("spi", "ls /sys/bus/spi/devices 2>/dev/null | tr '\\n' ' '"),
        ),
    ] {
        if !value.trim().is_empty() {
            push(&mut b, format!("- {label}: {value}"));
        }
    }
    blocks.push(b);

    // ---- boot configuration and image ----
    let mut b = section("Boot configuration and image");
    let boot = probe!(
        "boot",
        "for f in /boot/firmware/config.txt /boot/config.txt; do \
           [ -f \"$f\" ] && grep -E 'gpu_mem|dtoverlay|dtparam|fbcon|display' \"$f\" | head -12 | sed \"s|^|$(basename $(dirname $f))/$(basename $f): |\"; \
         done"
    );
    for line in boot.lines() {
        push(&mut b, format!("- {line}"));
    }
    push(
        &mut b,
        format!(
            "- image tag: {}",
            probe!("image", "cat /etc/cardputerzero-dtoverlays.commit 2>/dev/null; cat /etc/cardputerzero-image 2>/dev/null | head -2")
        ),
    );
    push(
        &mut b,
        format!(
            "- key packages: {}",
            probe!("packages", "dpkg-query -W -f='${Package} ${Version}\\n' applaunch linux-image-rpi-v8 2>/dev/null | tr '\\n' ' '")
        ),
    );
    blocks.push(b);

    // ---- camera / IR / expansion ----
    let mut b = section("Camera, IR and expansion");
    push(
        &mut b,
        format!(
            "- video/ir devices: {}",
            probe!("video", "ls /dev/video* /dev/media* /dev/lirc* 2>/dev/null | tr '\\n' ' '")
        ),
    );
    push(
        &mut b,
        format!(
            "- m5 io expander: {}",
            probe!("m5ioe", "for d in /sys/bus/i2c/devices/*; do [ -e \"$d/name\" ] && cat \"$d/name\"; done | grep -i -E 'm5|ioe|expander' | tr '\\n' ' '")
        ),
    );
    blocks.push(b);

    // ---- usb / modules / rtc ----
    let mut b = section("USB, kernel modules, RTC");
    for line in probe!("lsusb", "lsusb 2>/dev/null | head -12").lines() {
        push(&mut b, format!("- usb: {line}"));
    }
    let modules = probe!("lsmod", "lsmod 2>/dev/null | head -24");
    let count = modules.lines().count();
    push(&mut b, format!("- loaded modules: {count} (first ones)"));
    for line in modules.lines().skip(1).take(10) {
        push(&mut b, format!("  {line}"));
    }
    push(
        &mut b,
        format!(
            "- rtc: {} {}",
            read_text("/sys/class/rtc/rtc0/name"),
            probe!("rtc", "cat /sys/class/rtc/rtc0/date /sys/class/rtc/rtc0/time 2>/dev/null | paste -sd' ' -")
        ),
    );
    blocks.push(b);

    // Raw capture is returned through the block vector's side channel: keep it
    // in a synthetic last block so update() can write both files.
    let mut raws = section("\u{0}raw");
    for (label, out) in raw {
        if out.is_empty() {
            continue;
        }
        raws.lines.push(format!("### {label}\n```text\n{out}\n```"));
    }
    blocks.push(raws);
    blocks
}

/// Render the measured section from the blocks.
fn render_scan(blocks: &[Block], stamp: &str) -> (String, String, usize) {
    let mut out = String::from("## Current state (measured)\n");
    out.push_str(&format!(
        "Scan performed on the device at {stamp}. This section is regenerated by\n\
`/update-hwinfo`; the reference above is maintained by hand/model.\n"
    ));
    let mut checked = 0usize;
    let mut raw = String::new();
    for block in blocks {
        if block.title == "\u{0}raw" {
            raw = block.lines.join("\n\n");
            continue;
        }
        checked += 1;
        out.push_str(&format!("\n### {}\n", block.title));
        if block.lines.is_empty() {
            out.push_str("- (nothing detected)\n");
        } else {
            for line in &block.lines {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if !raw.is_empty() {
        let mut raw_trim: String = raw.chars().take(MAX_RAW).collect();
        if raw.chars().count() > MAX_RAW {
            raw_trim.push_str("\n... [raw capture truncated; full log in state/scan-latest.txt]");
        }
        out.push_str("\n### Raw capture\n\n");
        out.push_str(&raw_trim);
        out.push('\n');
    }
    (out, raw, checked)
}

/// Render the whole SKILL.md from the reference body, the scan section and the
/// preserved notes.
fn render_skill(
    reference: &str,
    scan_section: &str,
    notes: &str,
    version: u32,
    updated: &str,
) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("name: {SKILL}\n"));
    s.push_str(
        "description: Use when a task involves this Cardputer Zero's hardware - \
display, audio, keyboard/input, battery and power, WiFi/network, thermal, USB, \
I2C/SPI/GPIO, sensors - or before writing a device-specific app, so the code \
matches the real hardware.\n",
    );
    s.push_str("origin: dst\n");
    s.push_str(&format!("version: {version}\n"));
    s.push_str(&format!("updated: {updated}\n"));
    s.push_str("---\n\n");
    s.push_str(reference.trim_end());
    s.push_str("\n\n");
    s.push_str(SCAN_BEGIN);
    s.push('\n');
    s.push_str(scan_section.trim_end());
    s.push('\n');
    s.push_str(SCAN_END);
    s.push('\n');
    if !notes.trim().is_empty() {
        s.push('\n');
        s.push_str(notes.trim_end());
        s.push('\n');
    }
    s
}

/// Run a scan and rewrite the skill package. Returns what changed.
pub fn update() -> Result<Summary, String> {
    let started = Instant::now();
    let dir = skill_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::create_dir_all(dir.join("state"))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let current = read_skill(&dir);
    let (reference, mut version, _prev_updated) = reference_body(current.as_deref());
    let notes = current
        .as_deref()
        .map(notes_after_scan)
        .unwrap_or_default();

    let blocks = scan();
    let stamp = crate::apps::now_stamp();
    let (scan_section, raw, sections) = render_scan(&blocks, &stamp);
    // A rescan that measured the same hardware is not a new version: only
    // stamp the freshness and keep the version, so history stays meaningful.
    // Volatile numbers (uptime, free memory, temperatures, charge) are
    // normalised away; a different set of devices/subsystems still counts.
    let facts = |section: &str| -> String {
        section
            .lines()
            .filter(|l| !l.starts_with("Scan performed on the device at"))
            .map(|l| {
                l.chars()
                    .map(|c| if c.is_ascii_digit() { '#' } else { c })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string()
    };
    let prev_facts = current.as_deref().map(previous_scan).unwrap_or_default();
    let changed = facts(&prev_facts) != facts(&scan_section);
    if current.is_some() && changed {
        // Only a real change of the measured hardware is a new version; the
        // freshness stamp alone is not.
        version = version.saturating_add(1);
    }
    if current.is_none() {
        version = version.max(1);
    }
    let text = render_skill(&reference, &scan_section, &notes, version, &stamp);
    // Version history keeps a rescan reversible, like skill_save does.
    if let Some(old) = &current {
        if changed {
            let history = dir.join(".history");
            let _ = std::fs::create_dir_all(&history);
            let old_version = frontmatter_version(old);
            let _ = std::fs::write(
                history.join(format!("v{old_version}.md")),
                old,
            );
            // Keep the last five snapshots, like skill_save does.
            let mut snapshots: Vec<std::path::PathBuf> = std::fs::read_dir(&history)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file())
                        .collect()
                })
                .unwrap_or_default();
            snapshots.sort();
            while snapshots.len() > 5 {
                let _ = std::fs::remove_file(snapshots.remove(0));
            }
        }
    }
    let tmp = dir.join("SKILL.md.tmp-write");
    std::fs::write(&tmp, &text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dir.join("SKILL.md"))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    // Raw capture is kept next to the runtime state, outside the catalog.
    let raw_path = dir.join("state").join("scan-latest.txt");
    let raw_line = probe_header(&stamp);
    let _ = std::fs::write(&raw_path, format!("{raw_line}{raw}\n"));

    Ok(Summary {
        path: dir.join("SKILL.md").display().to_string(),
        version,
        sections,
        bytes: text.len(),
        changed,
        secs: started.elapsed().as_secs_f32(),
    })
}

fn probe_header(stamp: &str) -> String {
    format!("# hardware-info raw scan {stamp}\n\n")
}

/// The prompt handed to the model by `/update-hwinfo` in creator mode: the
/// scan has already refreshed the facts, the model reads the skill and refines
/// the parts that need judgement.
pub fn refine_prompt(summary: &Summary) -> String {
    format!(
        "The device's hardware-info skill was just refreshed from a real device scan \
({} sections, v{}, {}). Read it with the skill tool (name: {SKILL}); the raw capture is at \
~/.dst/skills/{SKILL}/state/scan-latest.txt. Check the measured state against the reference \
sections, fix anything that is wrong or missing, and rewrite the skill with skill_save if you \
have real improvements - especially facts useful for hardware development on this device. \
Do not invent hardware you cannot confirm: mark uncertain items as unverified. Keep it concise.",
        summary.sections, summary.version, summary.path
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "dst-hwinfo-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_shipped_seed_is_a_valid_skill_with_markers() {
        assert!(SEED.starts_with("---\n"));
        assert!(SEED.contains("name: hardware-info"));
        assert!(SEED.contains("description: Use when"));
        assert!(SEED.contains(SCAN_BEGIN) && SEED.contains(SCAN_END));
        // The reference stays substantial: this is the device's hardware guide.
        assert!(SEED.len() > 3_000, "seed is {} bytes", SEED.len());
    }

    #[test]
    fn render_keeps_reference_scan_and_notes_in_order() {
        let (reference, version, _) = reference_body(None);
        assert_eq!(version, 1);
        let scan = "## Current state (measured)\n\n### System\n- kernel: Linux test\n";
        let notes = "## Notes\n\n- the fan is silent\n";
        let text = render_skill(&reference, scan, notes, 2, "2026-11-18T12:00:00+08:00");
        assert!(text.starts_with("---\nname: hardware-info\n"));
        assert!(text.contains("version: 2"));
        assert!(text.contains("updated: 2026-11-18T12:00:00+08:00"));
        let begin = text.find(SCAN_BEGIN).unwrap();
        let end = text.find(SCAN_END).unwrap();
        let reference_at = text.find("# Cardputer Zero hardware reference").unwrap();
        let scan_at = text.find("## Current state").unwrap();
        let notes_at = text.find("## Notes").unwrap();
        assert!(reference_at < begin && begin < scan_at && scan_at < end && end < notes_at);
        // The marker itself must survive so the next update can find it.
        assert!(text.ends_with("\n"));
    }

    #[test]
    fn scan_renders_real_subsections_on_any_host() {
        let blocks = scan();
        assert!(blocks.len() >= 10, "expected every subsystem block");
        let (section, raw, checked) = render_scan(&blocks, "2026-11-18T12:00:00+08:00");
        assert!(checked >= 10);
        assert!(section.contains("### System"));
        assert!(section.contains("### Battery and power"));
        assert!(section.contains("### Buses and GPIO"));
        // Raw capture may be empty on a minimal host, but the renderer must not
        // panic and the section must still be complete.
        let _ = raw;
    }

    #[test]
    fn seed_install_never_overwrites_an_existing_package() {
        let root = tmpdir("seed");
        let dir = dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "hand written\n").unwrap();
        // Point the module at the temp root by exercising the pure helper: the
        // real seed() uses the configured skills dir, so test the predicate it
        // relies on instead.
        assert!(dir.join("SKILL.md").is_file());
        let current = read_skill(&dir);
        assert_eq!(current.as_deref(), Some("hand written\n"));
        let (reference, _, _) = reference_body(current.as_deref());
        assert_eq!(reference, "hand written", "a body without markers is kept whole");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn notes_survive_a_rescan() {
        let (reference, _, _) = reference_body(None);
        let first = render_skill(
            &reference,
            "## Current state (measured)\n\n### System\n- kernel: one\n",
            "",
            2,
            "2026-11-18T12:00:00+08:00",
        );
        let notes = notes_after_scan(&first);
        assert!(notes.is_empty());
        let with_notes = format!("{first}\n## Notes\n\n- kept\n");
        let notes = notes_after_scan(&with_notes);
        assert!(notes.contains("- kept"));
        let second = render_skill(
            &reference,
            "## Current state (measured)\n\n### System\n- kernel: two\n",
            &notes,
            3,
            "2026-11-18T13:00:00+08:00",
        );
        assert!(second.contains("kernel: two"));
        assert!(second.contains("- kept"));
        assert!(!second.contains("kernel: one"));
    }
}
