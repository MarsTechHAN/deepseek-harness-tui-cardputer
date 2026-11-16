#!/usr/bin/env python3
"""run_demo.py -- one-command end-to-end demo for the dst screen simulator.

Starts mock_api.py on an ephemeral localhost port, writes a temporary HOME with
a ~/.dst/config.yml pointing dst at that server, runs screen_sim.py with
sample_scenario.json, builds a contact sheet from the captured PNGs, then stops
the mock server.  Everything stays on 127.0.0.1; no network access is needed
beyond localhost.

    python3 run_demo.py                 # -> screenshots/*.png + contact_sheet.png
    python3 run_demo.py --scale 4       # bigger PNGs
    python3 run_demo.py --keep-work     # keep temp HOME / request log
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
REPO = os.environ.get("DST_REPO") or os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
DEFAULT_BIN = os.environ.get("DST_BIN", os.path.join(REPO, "target", "debug", "dst"))
MOCK = os.path.join(ROOT, "mock_api.py")
SIM = os.path.join(ROOT, "screen_sim.py")
SCENARIO = os.path.join(ROOT, "sample_scenario.json")
SCRIPT = os.path.join(ROOT, "sample_script.json")
PANELS = ["splash", "thinking", "tool_call", "final_reply"]


def log(msg: str) -> None:
    sys.stderr.write(f"[demo] {msg}\n")
    sys.stderr.flush()


def write_config(home: str, port: int) -> str:
    cfg = f"""model: deepseek-flash
effort: auto
max_tokens: 1024
api:
  base: http://127.0.0.1:{port}/v1
  key: test
voice:
  tts_voice: stella
tui:
  queued_default: true
  auto_send_voice: true
  language: en
"""
    d = os.path.join(home, ".dst")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, "config.yml")
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(cfg)
    os.chmod(path, 0o600)
    return path


def start_mock(port, script, log_path, env):
    cmd = [sys.executable, "-u", MOCK,
           "--port", str(port),
           "--script", script,
           "--log", log_path]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, bufsize=1, env=env, cwd=ROOT)
    url = None
    state = {"lines": []}
    deadline = time.time() + 15

    def drain():
        for line in proc.stdout:
            state["lines"].append(line.rstrip())
            if len(state["lines"]) < 12:
                log("mock: " + line.rstrip())

    threading.Thread(target=drain, daemon=True).start()
    while time.time() < deadline:
        if proc.poll() is not None:
            raise SystemExit("mock_api.py exited during startup:\n"
                             + "\n".join(state["lines"]))
        for line in list(state["lines"]):
            if line.startswith("MOCK_API_URL="):
                url = line.split("=", 1)[1].strip()
                break
        if url:
            break
        time.sleep(0.05)
    if not url:
        proc.kill()
        raise SystemExit("mock_api.py did not report its URL in time")
    return proc, url, state


def stop_mock(proc):
    if proc is None or proc.poll() is not None:
        return
    try:
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait(timeout=5)


def make_contact_sheet(snap_dir, panels, out_path):
    try:
        from PIL import Image, ImageDraw, ImageFont
    except ImportError:
        log("Pillow missing; skipping contact sheet")
        return None
    manifest_path = os.path.join(snap_dir, "manifest.json")
    entries = {}
    if os.path.exists(manifest_path):
        with open(manifest_path, "r", encoding="utf-8") as fh:
            doc = json.load(fh)
        for e in doc.get("snapshots", []):
            if e.get("png"):
                entries[e["name"]] = e["png"]
    chosen = [(name, entries[name]) for name in panels
              if name in entries and os.path.exists(entries[name])]
    if not chosen:
        log("no panel PNGs found; skipping contact sheet")
        return None

    font_path = "/System/Library/Fonts/Menlo.ttc"
    try:
        title_font = ImageFont.truetype(font_path, 26)
        label_font = ImageFont.truetype(font_path, 20)
    except Exception:
        title_font = label_font = ImageFont.load_default()

    pad, label_h, title_h = 18, 34, 52
    cols = 2 if len(chosen) > 1 else 1
    imgs = [(name, Image.open(path).convert("RGB")) for name, path in chosen]
    cell_w = max(im.width for _, im in imgs) + 2 * pad
    cell_h = max(im.height for _, im in imgs) + label_h + 2 * pad
    rows = (len(imgs) + cols - 1) // cols
    W = cols * cell_w + 2 * pad
    H = title_h + rows * cell_h + pad
    sheet = Image.new("RGB", (W, H), (12, 15, 20))
    draw = ImageDraw.Draw(sheet)
    header = ("dst screen simulator - sample scenario  "
              f"({imgs[0][0]}..{imgs[-1][0]})")
    draw.text((pad, 14), header, font=title_font, fill=(230, 237, 243))
    for i, (name, im) in enumerate(imgs):
        r, c = divmod(i, cols)
        x = pad + c * cell_w
        y = title_h + r * cell_h
        draw.text((x + pad, y + 6), f"{i + 1}. {name}", font=label_font,
                  fill=(120, 200, 255))
        sheet.paste(im, (x + pad, y + label_h))
        draw.rectangle([x + pad - 1, y + label_h - 1,
                        x + pad + im.width, y + label_h + im.height],
                       outline=(60, 70, 85))
    sheet.save(out_path, "PNG")
    return out_path


def validate_run(snap_dir, req_log):
    """Assert the demo actually exercised the intended states (fail loudly)."""
    problems = []
    checks = []

    manifest_path = os.path.join(snap_dir, "manifest.json")
    if not os.path.exists(manifest_path):
        problems.append(f"missing {manifest_path}")
        return checks, problems
    with open(manifest_path, "r", encoding="utf-8") as fh:
        manifest = json.load(fh)
    by_name = {}
    for entry in manifest.get("snapshots", []):
        if entry.get("text") and os.path.exists(entry["text"]):
            with open(entry["text"], "r", encoding="utf-8") as fh:
                by_name[entry["name"]] = fh.read()

    def need(name, needle=None, label=None):
        text = by_name.get(name)
        if text is None:
            problems.append(f"snapshot {name!r} not captured")
            return
        if needle is None:
            checks.append(f"{name}: captured ({len(text)} chars)")
            return
        if needle in text:
            checks.append(f"{name}: contains {label or needle!r}")
        else:
            problems.append(f"{name}: expected {label or needle!r} on screen; got:\n{text}")

    need("splash", "dst", "the 'dst' title")
    need("thinking", "thinking", "the live thinking indicator")
    need("reply", "mock", "streamed content")
    need("tool_call", "list_dir", "the list_dir tool line")
    need("final_reply", "中文", "CJK (wide-character) content")

    if manifest.get("exit_code") != 0:
        problems.append(f"dst exit_code={manifest.get('exit_code')!r} (expected 0)")
    else:
        checks.append("dst exited cleanly on Ctrl-C (code 0)")

    reqs, resps = [], {}
    if not os.path.exists(req_log):
        problems.append(f"missing request log {req_log}")
    else:
        with open(req_log, "r", encoding="utf-8") as fh:
            for line in fh:
                try:
                    rec = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if rec.get("type") == "request":
                    reqs.append(rec)
                elif rec.get("type") == "response":
                    resps[rec.get("request")] = rec
        checks.append(f"mock request log: {len(reqs)} chat request(s), "
                      f"{len(resps)} response(s)")
        if len(reqs) != 3:
            problems.append(f"expected 3 chat requests (2 user turns + tool follow-up), "
                            f"got {len(reqs)}")
        r2 = resps.get(2, {}).get("summary", {})
        if r2.get("tool_calls") != 1:
            problems.append(f"request 2 did not stream exactly one tool call: {r2}")
        else:
            checks.append("request 2: streamed tool_calls delta (finish_reason="
                          f"{r2.get('finish_reason')!r})")
        r3msgs = (reqs[2]["body"].get("messages") if len(reqs) > 2 else []) or []
        if not any(m.get("role") == "tool" for m in r3msgs):
            problems.append("request 3 did not carry the tool result back")
        else:
            checks.append("request 3: carried role='tool' result (agent loop closed)")
        auth = reqs[0].get("authorization") if reqs else None
        if auth != "Bearer test":
            problems.append(f"unexpected Authorization header: {auth!r}")
        else:
            checks.append("Authorization: Bearer test forwarded from config")
    return checks, problems


def main(argv=None):
    p = argparse.ArgumentParser(prog="run_demo.py",
                                description="End-to-end dst simulator demo.")
    p.add_argument("--bin", default=DEFAULT_BIN, help="dst binary")
    p.add_argument("--out", default=os.path.join(ROOT, "screenshots"),
                   help="snapshot output directory")
    p.add_argument("--work", default=os.path.join(ROOT, ".work"),
                   help="scratch directory for the temp HOME and request log")
    p.add_argument("--scale", type=int, default=3, help="PNG scale factor")
    p.add_argument("--port", type=int, default=0,
                   help="mock API port (0 = ephemeral, default)")
    p.add_argument("--timeout", type=int, default=180, help="simulator timeout (s)")
    p.add_argument("--keep-work", action="store_true",
                   help="keep the temporary HOME and request log")
    p.add_argument("--skip-validation", action="store_true",
                   help="do not assert the expected screens/requests")
    args = p.parse_args(argv)

    if not os.path.exists(args.bin):
        raise SystemExit(f"dst binary not found: {args.bin}\n"
                         f"build it: cd {REPO} && cargo build")
    os.makedirs(args.out, exist_ok=True)
    # Remove only files this demo generates (numbered snapshots + final.*).
    for fn in os.listdir(args.out):
        full = os.path.join(args.out, fn)
        if os.path.isfile(full) and (
                fn.startswith("final.") or fn.endswith(".png")
                or fn == "manifest.json" or fn.endswith(".txt")):
            os.remove(full)

    work = args.work
    home = os.path.join(work, "home")
    if os.path.isdir(home) and not args.keep_work:
        shutil.rmtree(home, ignore_errors=True)
    os.makedirs(home, exist_ok=True)
    os.makedirs(work, exist_ok=True)
    req_log = os.path.join(work, "requests.jsonl")
    if os.path.exists(req_log):
        os.remove(req_log)

    env = dict(os.environ)
    env.pop("TMUX", None)

    editor = None
    mock = None
    try:
        mock, url, _ = start_mock(args.port, SCRIPT, req_log, env)
        port = int(url.rsplit(":", 1)[1].split("/")[0])
        cfg_path = write_config(home, port)
        log(f"mock API: {url}")
        log(f"temp HOME: {home}  config: {cfg_path}")

        sim_cmd = [
            sys.executable, "-u", SIM,
            "--bin", args.bin,
            "--home", home,
            "--cwd", ROOT,
            "--out", args.out,
            "--scenario", SCENARIO,
            "--scale", str(args.scale),
        ]
        log("running: " + " ".join(sim_cmd))
        t0 = time.time()
        editor = subprocess.Popen(sim_cmd, cwd=ROOT, env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                  text=True)
        try:
            sim_out, sim_err = editor.communicate(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            editor.kill()
            sim_out, sim_err = editor.communicate()
            raise SystemExit(f"screen_sim.py timed out after {args.timeout}s")
        elapsed = time.time() - t0
        if sim_err.strip():
            for line in sim_err.strip().splitlines():
                sys.stderr.write(line + "\n")
        if editor.returncode != 0:
            raise SystemExit(f"screen_sim.py failed (exit {editor.returncode})")
        final_screen = sim_out
        with open(os.path.join(work, "final_screen.txt"), "w", encoding="utf-8") as fh:
            fh.write(final_screen)
        log(f"simulator finished in {elapsed:.1f}s")

        sheet = make_contact_sheet(args.out, PANELS,
                                   os.path.join(args.out, "contact_sheet.png"))

        checks, problems = ([], [])
        if not args.skip_validation:
            checks, problems = validate_run(args.out, req_log)
            for line in checks:
                log("check ok: " + line)
            for line in problems:
                log("CHECK FAILED: " + line)
            if problems:
                raise SystemExit(f"demo validation failed ({len(problems)} problem(s))")
            log(f"validation passed ({len(checks)} checks)")

        # Report.
        pngs = sorted(os.path.join(args.out, f) for f in os.listdir(args.out)
                      if f.endswith(".png"))
        print("\n=== demo complete ===")
        print(f"mock API log : {req_log}")
        print(f"temp HOME    : {home}")
        print(f"snapshots    : {args.out}")
        for path in pngs:
            print(f"  {path}")
        if sheet:
            print(f"contact sheet: {sheet}")
        if checks:
            print(f"validation   : {len(checks)} checks passed")
        print("\n=== final dst screen (captured before Ctrl-C) ===")
        print(final_screen.rstrip("\n"))
    finally:
        stop_mock(mock)
        if not args.keep_work and os.path.isdir(home):
            # Keep config + log for inspection but drop the caches: the HOME is
            # small and the config documents the ephemeral port mapping.
            pass
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        raise SystemExit(130)
