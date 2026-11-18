//! Voice input/output on DeepSeek's own APIs — pure Rust, no Python helper.
//!
//!   TTS: POST /v1/tools/tts   {text, voice_id, format:"pcm"}
//!        -> 24 kHz s16 mono PCM, converted here to 48 kHz S32 stereo for
//!           the ES8389 and piped into `aplay`.
//!   ASR: POST /v1/tools/asr   multipart file=<16 kHz mono s16 WAV>
//!        <- {text, duration_ms}
//!
//! Capture uses `arecord`; everything else (HTTP, resampling, WAV framing,
//! playback) is done in this file. No shelling out to Python or ffmpeg.

use crate::json::J;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const CARD: &str = "hw:CARD=ES8389Audio,DEV=0";
const MIC_RATE: usize = 48_000;
const ASR_RATE: usize = 16_000;

/// The codec needs a moment to unmute after the PCM is opened and drops the
/// frames written before that, which sounds like the first syllable is eaten.
const PLAY_PAD_MS: usize = 400;

static CFG: std::sync::Mutex<Option<crate::api::Cfg>> = std::sync::Mutex::new(None);

/// Set while `aplay` is running, so the UI can stop speech with Esc.
static PLAYING: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static STOP_PLAYBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask any in-flight TTS playback to stop (called when the user presses Esc).
pub fn request_stop() {
    // A clip may not be playing; log anyway so the simulator (and the device
    // journal) can prove an interrupt reached the voice layer.
    vlog(&format!(
        "stop requested (aplay pid={})",
        PLAYING.load(std::sync::atomic::Ordering::SeqCst)
    ));
    STOP_PLAYBACK.store(true, std::sync::atomic::Ordering::SeqCst);
    let pid = PLAYING.load(std::sync::atomic::Ordering::SeqCst);
    if pid > 0 {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
    }
}

pub fn set_cfg(c: crate::api::Cfg) {
    if let Ok(mut g) = CFG.lock() {
        *g = Some(c);
    }
}

fn cfg() -> Result<crate::api::Cfg, String> {
    CFG.lock()
        .ok()
        .and_then(|g| g.clone())
        .ok_or_else(|| "api config not initialised".to_string())
}

fn endpoint(path: &str) -> Result<String, String> {
    let c = cfg()?;
    let url = format!("{}/{}", c.base.trim_end_matches('/'), path);
    if !url.starts_with("https://") {
        return Err(format!("refusing non-HTTPS endpoint: {url}"));
    }
    Ok(url)
}

fn short(s: &str) -> String {
    s.chars().take(200).collect()
}

// ----------------------------------------------------------------------- TTS

/// Append one line to the voice log (also the evidence trail for the
/// simulator's mocked speech paths). The log rotates at 256 KB: every Esc/Tab
/// writes a line, and a device that runs for weeks must not grow it forever.
fn vlog(msg: &str) {
    use std::io::Write as _;
    let path = crate::config::log_dir().join("voice.log");
    if let Ok(m) = std::fs::metadata(&path) {
        if m.len() > 256 * 1024 {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Synthesise `text` and play it. Returns once playback has finished.
pub fn tts(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("nothing to say".into());
    }
    // Test mode: no speaker and no DeepSeek TTS API on a local vLLM endpoint.
    // Still take a moment so the running tool card is visible, and leave a log
    // line the simulator can assert on.
    if crate::config::mock_speech() {
        let ms: u64 = std::env::var("DST_MOCK_TTS_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(150);
        vlog(&format!(
            "mock tts {} chars, {} ms: {}",
            text.chars().count(),
            ms,
            short(text)
        ));
        std::thread::sleep(std::time::Duration::from_millis(ms));
        return Ok(());
    }
    let url = endpoint("tools/tts")?;
    let c = cfg()?;
    let body = crate::tool::obj(vec![
        ("text", J::s(text)),
        ("voice_id", J::s("stella")),
        ("format", J::s("pcm")),
    ])
    .dump();

    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "60",
            "-H",
            &crate::api::auth_header(&c),
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            &body,
            &url,
        ])
        .output()
        .map_err(|e| format!("curl: {e}"))?;

    let pcm = out.stdout;
    // An error response is small JSON, real audio is far bigger.
    if pcm.len() < 2_000 {
        return Err(format!(
            "tts: unexpected response: {}",
            short(&String::from_utf8_lossy(&pcm))
        ));
    }
    play_pcm24k(&pcm)
}

/// 24 kHz s16 mono -> 48 kHz S32 stereo (x2 upsample, sample<<16, duplicated
/// into both channels), with a leading silence pad.
fn play_pcm24k(pcm: &[u8]) -> Result<(), String> {
    let frames_in = pcm.len() / 2;
    let pad_frames = MIC_RATE * PLAY_PAD_MS / 1000;
    let mut raw: Vec<u8> = Vec::with_capacity(pad_frames * 8 + frames_in * 16);
    raw.extend(std::iter::repeat(0u8).take(pad_frames * 8));
    for i in 0..frames_in {
        let s = i16::from_le_bytes([pcm[2 * i], pcm[2 * i + 1]]) as i32;
        let v = (s << 16).to_le_bytes();
        for _ in 0..2 {
            raw.extend_from_slice(&v); // left
            raw.extend_from_slice(&v); // right
        }
    }

    let path = crate::config::cache_dir().join("tts.raw");
    let path = path.to_string_lossy().to_string();
    std::fs::write(&path, &raw).map_err(|e| format!("{path}: {e}"))?;
    STOP_PLAYBACK.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut child = Command::new("aplay")
        .args([
            "-q", "-D", CARD, "-f", "S32_LE", "-r", "48000", "-c", "2", "-t", "raw", path.as_str(),
        ])
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("aplay: {e}"))?;
    PLAYING.store(child.id() as i32, std::sync::atomic::Ordering::SeqCst);
    let mut interrupted = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
        }
        if STOP_PLAYBACK.swap(false, std::sync::atomic::Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            interrupted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    PLAYING.store(0, std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_file(&path);
    if interrupted {
        return Err("speech interrupted by the user".into());
    }
    Ok(())
}

// ----------------------------------------------------------------------- ASR

pub struct Recorder {
    child: Option<std::process::Child>,
    buf: Arc<Mutex<Vec<i16>>>,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    /// Mocked take: no `arecord` child and no upload; `finish` returns a
    /// canned transcript instead (see `DST_MOCK_SPEECH`).
    mock: bool,
}

/// Hard cap on one take: a push-to-talk take is capped at 30 s in the UI, but
/// an interrupt take can stay open for a whole (unbounded) turn, and 48 kHz
/// mono is 96 KB/s. 60 s is generous for speech and keeps the worst case at
/// ~6 MB instead of tens of MB on the 446 MB device.
pub const MAX_TAKE_SAMPLES: usize = 48_000 * 60;

/// Start capturing from the microphone. Call `finish()` to transcribe, or
/// `cancel()` to throw the take away.
pub fn start_recording() -> Result<Recorder, String> {
    if crate::config::mock_speech() {
        vlog("mock recording started");
        return Ok(Recorder {
            child: None,
            buf: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
            handle: None,
            mock: true,
        });
    }
    let mut child = Command::new("arecord")
        .args([
            "-q", "-D", CARD, "-f", "S32_LE", "-r", "48000", "-c", "2", "-t", "raw",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("arecord: {e}"))?;

    let mut stdout = child.stdout.take().ok_or("arecord: no stdout")?;
    let buf = Arc::new(Mutex::new(Vec::<i16>::with_capacity(ASR_RATE * 30)));
    let stop = Arc::new(AtomicBool::new(false));
    let b = buf.clone();
    let st = stop.clone();

    let handle = std::thread::spawn(move || {
        // 48 kHz, 2ch, S32 => 8 bytes per frame. Read in ~20 ms blocks.
        let mut chunk = [0u8; 7_680];
        loop {
            if st.load(Ordering::Relaxed) {
                break;
            }
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    let mut v = b.lock().unwrap();
                    // A recording whose owner forgot to stop it must not eat
                    // the device's RAM: stop buffering at the take cap.
                    if v.len() >= MAX_TAKE_SAMPLES {
                        break;
                    }
                    let room = MAX_TAKE_SAMPLES - v.len();
                    for i in 0..(n / 8).min(room) {
                        let o = i * 8;
                        let s = i32::from_le_bytes([
                            chunk[o],
                            chunk[o + 1],
                            chunk[o + 2],
                            chunk[o + 3],
                        ]);
                        v.push((s >> 16) as i16); // mono: left channel
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(Recorder {
        child: Some(child),
        buf,
        stop,
        handle: Some(handle),
        mock: false,
    })
}

impl Recorder {
    fn stop_capture(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }

    /// Stop, convert to 16 kHz mono WAV and upload. Returns the transcript.
    pub fn finish(mut self) -> Result<String, String> {
        self.stop_capture();
        if self.mock {
            std::thread::sleep(std::time::Duration::from_millis(120));
            let text = crate::config::mock_asr_text();
            vlog(&format!("mock asr: {text:?}"));
            return Ok(text);
        }
        let samples = {
            let v = self.buf.lock().unwrap();
            v.clone()
        };
        if samples.len() < MIC_RATE / 2 {
            return Err("recording was too short".into());
        }
        // 48 kHz -> 16 kHz by dropping two of every three samples.
        let mono: Vec<i16> = samples.iter().step_by(MIC_RATE / ASR_RATE).cloned().collect();
        let wav = to_wav(&mono, ASR_RATE as u32);

        let path = crate::config::cache_dir().join("asr.wav");
        let path = path.to_string_lossy().to_string();
        std::fs::write(&path, &wav).map_err(|e| format!("{path}: {e}"))?;
        let result = upload_asr(&path);
        let _ = std::fs::remove_file(&path);
        result
    }

    pub fn cancel(mut self) {
        self.stop_capture();
    }
}

/// Never leave `arecord` running if a Recorder is dropped without finishing
/// (e.g. the turn was interrupted again while the take was being handed over).
impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop_capture();
    }
}

fn to_wav(samples: &[i16], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

fn upload_asr(path: &str) -> Result<String, String> {
    let url = endpoint("tools/asr")?;
    let c = cfg()?;
    let out = Command::new("curl")
        .args([
            "-sS",
            "--max-time",
            "120",
            "-H",
            &crate::api::auth_header(&c),
            "--form",
            &format!("file=@{path}"),
            &url,
        ])
        .output()
        .map_err(|e| format!("curl: {e}"))?;
    let body = String::from_utf8_lossy(&out.stdout).to_string();
    if body.trim().is_empty() {
        return Err(format!(
            "asr: empty response ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let v = crate::json::parse(&body)
        .ok_or_else(|| format!("asr: bad JSON: {}", short(&body)))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("asr request failed");
        return Err(msg.to_string());
    }
    Ok(v.get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string())
}

/// Microphone availability. Must use the format the codec actually exposes
/// (48 kHz S32 stereo); asking for 16 kHz mono fails on this hardware.
#[allow(dead_code)] // diagnostics helper used by --voice-selftest
pub fn mic_ok() -> bool {
    Command::new("arecord")
        .args([
            "-q", "-D", CARD, "-f", "S32_LE", "-r", "48000", "-c", "2", "-d", "1", "-t", "raw",
            "/dev/null",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

impl Recorder {
    /// Normalised RMS of what has been captured so far (0..1).
    pub fn level(&self) -> f64 {
        let v = self.buf.lock().unwrap();
        if v.is_empty() {
            return 0.0;
        }
        let sum: f64 = v.iter().map(|s| (*s as f64).powi(2)).sum();
        (sum / v.len() as f64).sqrt() / 32768.0
    }
}

/// Record `secs`, report the normalised RMS level and the transcript.
/// Used by `dst --voice-selftest` to separate "mic is dead" from "ASR decided
/// the take was noise".
pub fn mic_probe(secs: u32) -> Result<(f64, String), String> {
    if crate::config::mock_speech() {
        std::thread::sleep(std::time::Duration::from_millis(secs.min(1) as u64 * 200));
        return Ok((0.25, crate::config::mock_asr_text()));
    }
    let mut rec = start_recording()?;
    std::thread::sleep(std::time::Duration::from_secs(secs as u64));
    rec.stop_capture();
    let samples = {
        let v = rec.buf.lock().unwrap();
        v.clone()
    };
    if samples.is_empty() {
        return Err("no audio captured".into());
    }
    let rms = (samples.iter().map(|s| (*s as f64).powi(2)).sum::<f64>()
        / samples.len() as f64)
        .sqrt();
    let mono: Vec<i16> = samples.iter().step_by(MIC_RATE / ASR_RATE).cloned().collect();
    let wav = to_wav(&mono, ASR_RATE as u32);
    let path = crate::config::cache_dir().join("probe.wav");
    let path = path.to_string_lossy().to_string();
    std::fs::write(&path, &wav).map_err(|e| format!("{path}: {e}"))?;
    let text = upload_asr(&path);
    let _ = std::fs::remove_file(&path);
    Ok((rms / 32768.0, text?))
}
