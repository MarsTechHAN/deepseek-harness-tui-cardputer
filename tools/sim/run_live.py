#!/usr/bin/env python3
"""run_live.py -- visual UX walkthrough against the real local vLLM endpoint.

Unlike ``run_scenarios.py`` (which drives dst against the scripted mock API),
this walkthrough uses the live model from the DSH config, so the transcript
content is a real generation: real reasoning, real streaming, real tool calls
and real latency. Only speech is mocked (``DST_MOCK_SPEECH=1``): the
simulator host has no microphone/speaker and a local vLLM endpoint has no
DeepSeek ASR/TTS APIs. ASR returns ``DST_MOCK_ASR_TEXT`` and TTS logs instead
of playing, which is enough to review the voice UI.

The walkthrough covers the common usage path, capturing one PNG per step:

    01_startup        first-run tips (tagline + key tips)
    02_streaming      live reasoning while the answer streams
    03_reply          settled first answer
    04_tool_running   a tool card while it runs
    05_tool_done      tool result card + final answer
    06_recording      push-to-talk REC indicator
    07_voice_stream   auto-sent voice prompt, model streaming
    08_voice_reply    settled voice answer
    09_help           /help menu
    10_config         /config wizard
    11_resume         /resume session picker
    12_login          /login entry menu

Endpoint discovery: ``DST_LIVE_BASE``/``DST_LIVE_MODEL`` env vars, else the
first ``baseURL`` and the ``agent-default-model`` model in
``~/.dsh/settings.yaml`` (or settings.yaml.imported).

    python3 run_live.py                  # full walkthrough
    python3 run_live.py --out /tmp/live  # screenshots elsewhere
    python3 run_live.py --quick          # skip the tool and voice turns

Exit status is non-zero only for hard failures (missing snapshot, menu not
rendered, endpoint unreachable); content checks are reported as warnings
because a live model is free to answer differently.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
SIM = os.path.join(HERE, "screen_sim.py")
DEFAULT_BIN = os.environ.get("DST_BIN", os.path.join(ROOT, "target", "debug", "dst"))
DEFAULT_OUT = os.path.join(HERE, "screenshots", "live")
DEFAULT_DOC = os.path.join(ROOT, "docs", "live-walkthrough.png")

PANELS = [
    "01_startup", "02_streaming", "03_reply", "04_tool_running",
    "05_tool_done", "06_recording", "07_voice_stream", "08_voice_reply",
    "09_help", "10_config", "11_resume", "12_login",
]

ASR_TEXT = ("In three sentences, explain why push-to-talk is useful on a tiny terminal.")


def log(msg):
    print(f"[live] {msg}", flush=True)


def endpoint_label(base):
    """Contact-sheet label for an endpoint, with address literals redacted.

    The walkthrough can run against a private devbox. The committed PNG in
    docs/ must not carry that address, so a bare IPv4/IPv6 host is replaced
    with a generic label; hostnames are kept (truncated).
    """
    host = base.split("//")[-1].split("/")[0]
    host = host.rsplit("@", 1)[-1]
    bare = host.startswith("[") or re.fullmatch(r"[0-9.]+(:\d+)?", host) \
        or re.fullmatch(r"[0-9a-fA-F:]{4,}(:\d+)?", host)
    return "local vLLM" if bare else host[:40]


# --------------------------------------------------------------------------- #
# endpoint discovery
# --------------------------------------------------------------------------- #

def dsh_endpoint():
    base = os.environ.get("DST_LIVE_BASE", "").strip()
    model = os.environ.get("DST_LIVE_MODEL", "").strip()
    if base and model:
        return base, model
    for name in ("settings.yaml", "settings.yaml.imported"):
        path = os.path.expanduser(os.path.join("~/.dsh", name))
        if not os.path.exists(path):
            continue
        text = open(path, encoding="utf-8", errors="replace").read()
        if not base:
            m = re.search(r"baseURL:\s*[\"']?(\S+?)[\"']?\s*$", text, re.MULTILINE)
            if m:
                base = m.group(1)
        if not model:
            m = re.search(
                r"agent-default-model:[\s\S]*?\n\s+model:\s*[\"']?([^\s\"']+)", text)
            if m:
                model = m.group(1)
        if base:
            break
    return base, model or "deepseek-v4.1-flash"


def probe_endpoint(base, timeout=8):
    """Return the model ids advertised by /v1/models, or raise."""
    url = base.rstrip("/") + "/models"
    r = subprocess.run(
        ["curl", "-sS", "--max-time", str(timeout), url],
        capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"cannot reach {url}: {r.stderr.strip()}")
    try:
        doc = json.loads(r.stdout)
    except json.JSONDecodeError:
        raise RuntimeError(f"{url} did not return JSON: {r.stdout[:200]!r}")
    ids = [m.get("id") for m in doc.get("data") or [] if m.get("id")]
    if not ids:
        raise RuntimeError(f"{url} lists no models")
    return ids


# --------------------------------------------------------------------------- #
# config + scenario
# --------------------------------------------------------------------------- #

def write_home(home, base, model, effort, language="en"):
    dst_dir = os.path.join(home, ".dst")
    os.makedirs(dst_dir, exist_ok=True)
    cfg = f"""model: {model}
effort: {effort}
max_tokens: 2048
api:
  base: {base}
  key: live-test-key
voice:
  tts_voice: stella
tui:
  queued_default: true
  auto_send_voice: true
  language: {language}
context:
  time: false
"""
    path = os.path.join(dst_dir, "config.yml")
    open(path, "w", encoding="utf-8").write(cfg)
    os.chmod(path, 0o600)
    return home


def hold_tab_steps(hold_repeats=3):
    """Emulate a long press: the terminal sends TAB repeats every ~250 ms.

    dst arms after three repeats and ends the take when the repeats stop for
    300 ms, so the recording must stay above the 0.6 s noise floor.
    """
    steps = [{"key": "Tab"}]
    for _ in range(3):                       # arm the recorder
        steps += [{"wait_ms": 150}, {"key": "Tab"}]
    for _ in range(hold_repeats):            # keep holding
        steps += [{"wait_ms": 150}, {"key": "Tab"}]
    steps += [{"wait_ms": 150}]
    return steps


def build_scenario(quick):
    steps = [
        {"wait_for": {"text": "Tab", "timeout_ms": 20000}},
        {"snap": "01_startup"},
        {"send": "In three short sentences, explain what a pocket AI terminal is for."},
        {"key": "Enter"},
        {"wait_ms": 800, "snap": "02_streaming"},
        {"wait_stable": {"stable_ms": 1400, "timeout_ms": 120000}, "snap": "03_reply"},
    ]
    if not quick:
        steps += [
            {"send": "Use the bash tool to run: sleep 2; printf 'alpha\\nbeta\\n'. Then tell me exactly what it printed."},
            {"key": "Enter"},
            {"wait_for": {"text": "bash >", "timeout_ms": 60000}},
            {"wait_ms": 350, "snap": "04_tool_running"},
            {"wait_stable": {"stable_ms": 1400, "timeout_ms": 150000}, "snap": "05_tool_done"},
        ]
        steps += hold_tab_steps()
        steps += [
            {"key": "Tab", "snap": "06_recording"},
            {"wait_ms": 600, "snap": "07_voice_stream"},
            {"wait_stable": {"stable_ms": 1400, "timeout_ms": 150000}, "snap": "08_voice_reply"},
        ]
    steps += [
        {"send": "/help"},
        {"key": "Enter"},
        {"wait_ms": 500, "snap": "09_help"},
        {"key": "Escape"},
        {"wait_ms": 300},
        {"send": "/config"},
        {"key": "Enter"},
        {"wait_ms": 500, "snap": "10_config"},
        {"key": "Escape"},
        {"wait_ms": 300},
        {"send": "/resume"},
        {"key": "Enter"},
        {"wait_ms": 800, "snap": "11_resume"},
        {"key": "Escape"},
        {"wait_ms": 300},
        {"send": "/login"},
        {"key": "Enter"},
        {"wait_ms": 800, "snap": "12_login"},
        {"key": "Escape"},
        {"wait_ms": 300},
        {"key": "CtrlC"},
    ]
    return {"description": "live vLLM UX walkthrough", "steps": steps}


# --------------------------------------------------------------------------- #
# screenshots / contact sheet
# --------------------------------------------------------------------------- #

def read_texts(out_dir):
    """Map snapshot names (as passed to `snap`) to their rendered text."""
    texts = {}
    manifest_path = os.path.join(out_dir, "manifest.json")
    if not os.path.exists(manifest_path):
        return texts
    doc = json.load(open(manifest_path, encoding="utf-8"))
    for e in doc.get("snapshots", []):
        name, path = e.get("name"), e.get("text")
        if name and path and os.path.exists(path):
            texts[name] = open(path, encoding="utf-8", errors="replace").read()
    return texts


def make_contact_sheet(out_dir, panels, path, title, cols=3):
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        log("Pillow missing; skipping contact sheet")
        return None
    manifest_path = os.path.join(out_dir, "manifest.json")
    entries = {}
    if os.path.exists(manifest_path):
        doc = json.load(open(manifest_path, encoding="utf-8"))
        for e in doc.get("snapshots", []):
            if e.get("png"):
                entries[e["name"]] = e["png"]
    chosen = [(n, entries[n]) for n in panels if n in entries and os.path.exists(entries[n])]
    if not chosen:
        return None
    font = "/System/Library/Fonts/Menlo.ttc"
    try:
        title_font = ImageFont.truetype(font, 24)
        label_font = ImageFont.truetype(font, 18)
    except Exception:
        title_font = label_font = ImageFont.load_default()
    pad, label_h, title_h = 14, 28, 44
    imgs = [(n, Image.open(p).convert("RGB")) for n, p in chosen]
    cell_w = max(im.width for _, im in imgs) + 2 * pad
    cell_h = max(im.height for _, im in imgs) + label_h + 2 * pad
    rows = (len(imgs) + cols - 1) // cols
    W = cols * cell_w + 2 * pad
    H = title_h + rows * cell_h + pad
    sheet = Image.new("RGB", (W, H), (12, 15, 20))
    draw = ImageDraw.Draw(sheet)
    draw.text((pad, 10), title, font=title_font, fill=(230, 237, 243))
    for i, (name, im) in enumerate(imgs):
        r, c = divmod(i, cols)
        x = pad + c * cell_w
        y = title_h + r * cell_h
        draw.text((x + pad + 4, y + 2), name, font=label_font, fill=(120, 200, 255))
        sheet.paste(im, (x + pad, y + label_h))
        draw.rectangle([x + pad - 1, y + label_h - 1,
                        x + pad + im.width, y + label_h + im.height],
                       outline=(60, 70, 85))
    sheet.save(path, "PNG")
    return path


# --------------------------------------------------------------------------- #
# validation
# --------------------------------------------------------------------------- #

def validate(texts, quick):
    hard, soft = [], []

    def need(cond, label):
        if not cond:
            hard.append(label)

    def nice(cond, ok_label, bad_label):
        soft.append((ok_label if cond else bad_label, cond))

    for name in PANELS:
        if quick and name not in ("01_startup", "02_streaming", "03_reply",
                                  "09_help", "10_config", "11_resume", "12_login"):
            continue
        need(name in texts, f"missing snapshot {name}")
    if hard:
        return hard, soft

    startup = texts["01_startup"]
    nice("dst" in startup, "startup shows the tagline",
         "startup screen has no tagline")

    reply = texts["03_reply"]
    nice(reply.count("\n") > 3, "first answer rendered", "first answer looks empty")
    nice("thinking" in reply or "reasoning" in reply,
         "reasoning line present", "no folded reasoning line in the settled screen")

    help_menu = texts["09_help"]
    need("/config" in help_menu and "Enter run" in help_menu,
         "/help menu did not list commands or its hint")
    cfg = texts["10_config"]
    need(any(k in cfg for k in ("Language", "language", "Sandbox", "sandbox")),
         "/config wizard did not render its rows")
    resume = texts["11_resume"]
    need("Resume" in resume or "resum" in resume.lower(),
         "/resume picker did not render")
    login = texts["12_login"]
    need("WeChat" in login or "DeepSeek" in login, "/login menu did not render")

    if not quick:
        rec = texts["06_recording"]
        need("REC" in rec, "push-to-talk did not show the REC indicator")
        # The settled screen only keeps the transcript tail, so look for the
        # end of the mocked dictation rather than its first words.
        voice = " ".join(texts["08_voice_reply"].split())
        voice_tail = " ".join(ASR_TEXT.split()[-5:])
        nice(voice_tail in voice, "voice prompt visible in transcript",
             "mocked ASR transcript not visible")
        tool = texts["05_tool_done"]
        nice("bash" in tool, "tool card rendered",
             "the model answered the bash request without a tool call")
        nice("alpha" in tool, "tool output visible", "tool output preview missing")
    return hard, soft


# --------------------------------------------------------------------------- #
# main
# --------------------------------------------------------------------------- #

def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--bin", default=DEFAULT_BIN)
    ap.add_argument("--base", default="", help="OpenAI-compatible base URL")
    ap.add_argument("--model", default="", help="model id")
    ap.add_argument("--effort", default="high",
                    help="reasoning effort (default high; low disables reasoning on vLLM)")
    ap.add_argument("--out", default=DEFAULT_OUT, help="screenshot dir")
    ap.add_argument("--doc", default=DEFAULT_DOC, help="contact sheet PNG for docs")
    ap.add_argument("--scale", type=int, default=2)
    ap.add_argument("--quick", action="store_true",
                    help="skip the tool and voice turns (fast smoke run)")
    ap.add_argument("--keep-home", action="store_true")
    args = ap.parse_args(argv)

    if not os.path.exists(args.bin):
        raise SystemExit(f"dst binary not found: {args.bin} (run cargo build)")

    base, model = dsh_endpoint()
    base = args.base or base
    model = args.model or model
    if not base:
        raise SystemExit("no endpoint: set DST_LIVE_BASE/DST_LIVE_MODEL or "
                         "configure ~/.dsh/settings.yaml")
    log(f"endpoint {base}  model {model}  effort {args.effort}")
    try:
        ids = probe_endpoint(base)
    except RuntimeError as exc:
        raise SystemExit(f"live endpoint unreachable: {exc}")
    if model not in ids:
        log(f"warning: model {model!r} not advertised ({', '.join(ids[:4])})")

    work = tempfile.mkdtemp(prefix="dst-live-")
    home = write_home(os.path.join(work, "home"), base, model, args.effort)
    cwd = os.path.join(work, "work")
    os.makedirs(cwd, exist_ok=True)
    out_dir = args.out
    if os.path.isdir(out_dir):
        shutil.rmtree(out_dir)
    os.makedirs(out_dir, exist_ok=True)
    scenario_path = os.path.join(work, "live.json")
    json.dump(build_scenario(args.quick), open(scenario_path, "w"),
              ensure_ascii=False, indent=1)

    env = dict(os.environ)
    env.update({
        # Only speech is mocked; the LLM is the real local vLLM endpoint.
        "DST_MOCK_SPEECH": "1",
        "DST_MOCK_ASR_TEXT": ASR_TEXT,
        "DST_MOCK_TTS_MS": "150",
    })
    cmd = [sys.executable, SIM, "--bin", os.path.abspath(args.bin),
           "--home", home, "--cwd", cwd, "--out", out_dir,
           "--scenario", scenario_path, "--scale", str(args.scale)]
    t0 = time.time()
    r = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True, text=True,
                       timeout=600)
    for line in (r.stdout + r.stderr).splitlines():
        if line.startswith("[sim]"):
            log(line)
    if r.returncode != 0:
        print(r.stderr[-3000:])
        raise SystemExit(f"simulator failed (exit {r.returncode})")

    texts = read_texts(out_dir)
    panels = [p for p in PANELS if p in texts] if args.quick else PANELS
    sheet = make_contact_sheet(
        out_dir, panels,
        os.path.join(out_dir, "contact_sheet.png"),
        f"dst live walkthrough - {model} @ {endpoint_label(base)}  ({time.time()-t0:.0f}s)")
    if sheet and args.doc:
        os.makedirs(os.path.dirname(args.doc), exist_ok=True)
        shutil.copyfile(sheet, args.doc)
        log(f"docs contact sheet -> {args.doc}")

    hard, soft = validate(texts, args.quick)
    for label, ok in soft:
        log(("ok   " if ok else "warn ") + label)
    for label in hard:
        log("FAIL " + label)
    shots = len([f for f in os.listdir(out_dir) if f.endswith(".png")])
    log(f"{shots} screenshots in {out_dir} ({time.time()-t0:.0f}s)")
    if args.keep_home:
        log(f"home kept at {home}")
    else:
        shutil.rmtree(work, ignore_errors=True)
    if hard:
        log(f"{len(hard)} hard check(s) failed")
        return 1
    log("walkthrough complete")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
