#!/usr/bin/env python3
"""check_multi_instance.py -- two dst instances may run side by side.

dst used to be a machine-wide singleton (`/tmp/dst.lock/pid`). That blocked a
perfectly valid setup: the panel instance on tty1, an SSH session, and a tmux
pane each own their own pty and cannot steal keys from each other. This check
starts two real dst instances concurrently against the same HOME and asserts
that both start and render; only a second instance on the *same* terminal is
still refused (unit-tested in src/main.rs).

    python3 check_multi_instance.py
"""

from __future__ import annotations

import glob
import os
import subprocess
import sys
import tempfile

from run_demo import DEFAULT_BIN, SIM, start_mock, stop_mock


def main() -> int:
    work = tempfile.mkdtemp(prefix="dst-multi-")
    home = os.path.join(work, "home")
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    script = os.path.join(work, "script.json")
    with open(script, "w", encoding="utf-8") as fh:
        fh.write('{"responses": []}\n')
    mock, url, _state = start_mock(0, script, os.path.join(work, "requests.jsonl"),
                                   dict(os.environ))
    env = dict(os.environ)
    outs = []
    procs = []
    runs = []
    try:
        cfg = (
            "model: deepseek-flash\neffort: auto\nmax_tokens: 256\n"
            f"api:\n  base: {url}\n  key: test\n"
            "context:\n  time: false\ntui:\n  language: en\n"
        )
        cfg_path = os.path.join(home, ".dst", "config.yml")
        with open(cfg_path, "w", encoding="utf-8") as fh:
            fh.write(cfg)
        os.chmod(cfg_path, 0o600)
        for i in range(2):
            out = os.path.join(work, f"out{i}")
            outs.append(out)
            cmd = [
                sys.executable, SIM, "--bin", DEFAULT_BIN, "--home", home,
                "--cwd", work, "--out", out, "--text-only",
                "--wait-ms", "2500", "--snap", f"boot{i}",
            ]
            runs.append(subprocess.Popen(
                cmd, cwd=os.path.dirname(os.path.abspath(__file__)),
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                env=env))
        results = []
        for proc in runs:
            try:
                output, _ = proc.communicate(timeout=40)
            except subprocess.TimeoutExpired:
                proc.kill()
                output, _ = proc.communicate()
                output += "\n(timed out)"
            results.append((proc.returncode, output or ""))
    finally:
        stop_mock(mock)

    checks = []
    for i, (code, output) in enumerate(results):
        snaps = glob.glob(os.path.join(outs[i], f"*boot{i}.txt"))
        text = ""
        if snaps:
            with open(snaps[0], encoding="utf-8", errors="replace") as fh:
                text = fh.read()
        checks.append((code == 0, f"instance {i + 1} exited cleanly (code {code})"))
        checks.append(("dst" in text.lower(),
                       f"instance {i + 1} rendered the TUI on its own terminal"))
        checks.append(("already runs a dst instance" not in output,
                       f"instance {i + 1} was not blocked by another instance"))
    ok = all(passed for passed, _ in checks)
    for passed, label in checks:
        print(f"  {'ok  ' if passed else 'FAIL'} {label}")
    print(f"[{'PASS' if ok else 'FAIL'}] multi_instance")
    if not ok or os.environ.get("DST_KEEP_WORK"):
        print(f"  work dir kept: {work}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
