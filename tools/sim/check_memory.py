#!/usr/bin/env python3
"""check_memory.py -- drive a long session through the simulator and sample RSS.

The device has 512 MB of RAM, so the TUI must not let the transcript or the
message serialisation grow without bound. This check runs a 20-turn session
with large replies, samples the dst child's resident set size, and asserts the
peak stays far below the device budget while the last turn still renders.

    python3 check_memory.py            # build target/debug/dst first
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

from run_demo import DEFAULT_BIN, REPO, SIM, log, start_mock, stop_mock

ROOT = os.path.dirname(os.path.abspath(__file__))
TURNS = 20
PEAK_LIMIT_MB = 80.0  # debug binary + pty/output buffers; device budget is 512 MB


def child_rss_mb(pid):
    try:
        out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                             capture_output=True, text=True, timeout=5)
        kb = int(out.stdout.strip() or 0)
        return kb / 1024.0
    except (ValueError, subprocess.SubprocessError):
        return 0.0


def build_fixtures(work):
    home = os.path.join(work, "home")
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    out = os.path.join(work, "out")
    os.makedirs(out, exist_ok=True)
    scenario = os.path.join(work, "scenario.json")
    script = os.path.join(work, "script.json")

    steps = [{"wait_for": {"text": "dst", "timeout_ms": 15000}}]
    responses = []
    filler = ("Turn answer with a long body so the transcript grows past its "
              "retention bound and the message serialiser has real work to do. ")
    for i in range(1, TURNS + 1):
        steps += [{"send": f"question {i}"}, {"key": "Enter"}]
        if i == TURNS:
            steps.append({"wait_ms": 250, "snap": "last_echo"})
        steps.append({"wait_for": {"text": f"Turn {i} answer", "timeout_ms": 20000}})
        # Let the turn finish before queuing the next question: sending while
        # busy queues the text instead of starting the next turn.
        steps.append({"wait_stable": {"stable_ms": 500, "timeout_ms": 15000}})
        responses.append({"reasoning": f"turn {i}", "content": f"Turn {i} answer. " + filler * 6,
                          "finish_reason": "stop"})
    steps.append({"wait_stable": {"stable_ms": 800, "timeout_ms": 15000}, "snap": "final"})
    json.dump({"steps": steps}, open(scenario, "w"), ensure_ascii=False, indent=1)
    json.dump({"responses": responses}, open(script, "w"), ensure_ascii=False, indent=1)
    return home, out, scenario, script


def main():
    binary = os.environ.get("DST_BIN", DEFAULT_BIN)
    work = tempfile.mkdtemp(prefix="dst-memory-")
    try:
        home, out, scenario, script = build_fixtures(work)
        log_path = os.path.join(work, "requests.jsonl")
        env = dict(os.environ)
        proc, url, _state = start_mock(0, script, log_path, env)
        pid_file = os.path.join(work, "dst.pid")
        try:
            cfg = (f"model: deepseek-flash\neffort: auto\nmax_tokens: 1024\n"
                   f"api:\n  base: {url}\n  key: test\n"
                   f"compaction:\n  enabled: false\ncontext:\n  time: false\n")
            cfg_path = os.path.join(home, ".dst", "config.yml")
            open(cfg_path, "w", encoding="utf-8").write(cfg)
            os.chmod(cfg_path, 0o600)
            cmd = [sys.executable, SIM, "--bin", binary, "--home", home,
                   "--cwd", work, "--out", out, "--scenario", scenario,
                   "--pid-file", pid_file, "--text-only"]
            sim = subprocess.Popen(cmd, cwd=ROOT, env=env,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            peak = 0.0
            samples = 0
            while sim.poll() is None:
                if os.path.exists(pid_file):
                    try:
                        pid = int(open(pid_file).read().strip())
                    except (ValueError, OSError):
                        pid = 0
                    rss = child_rss_mb(pid) if pid else 0.0
                    if rss > 0:
                        peak = max(peak, rss)
                        samples += 1
                time.sleep(0.1)
            sim.wait(timeout=10)
        finally:
            stop_mock(proc)

        final = ""
        echo = ""
        for name in os.listdir(out):
            path = os.path.join(out, name)
            if "last_echo" in name:
                echo = open(path, encoding="utf-8").read()
            elif name.startswith("01_final") or name.startswith("final"):
                final = open(path, encoding="utf-8").read()
        checks = [
            (sim.returncode == 0, f"simulator exited 0 (got {sim.returncode})"),
            (f"> question {TURNS}" in echo, f"turn {TURNS} echo visible after a long session"),
            (bool(final), "final screen captured"),
            (samples > 10, f"RSS sampled {samples} times"),
            (0 < peak < PEAK_LIMIT_MB,
             f"peak RSS {peak:.1f} MB < {PEAK_LIMIT_MB:.0f} MB ({samples} samples)"),
        ]
        for ok, label in checks:
            print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        passed = all(ok for ok, _ in checks)
        print(f"[{'PASS' if passed else 'FAIL'}] memory")
        return 0 if passed else 1
    finally:
        if passed or not os.environ.get("DST_KEEP_WORK"):
            shutil.rmtree(work, ignore_errors=True)
        else:
            print(f"work dir kept: {work}")


if __name__ == "__main__":
    sys.exit(main())
