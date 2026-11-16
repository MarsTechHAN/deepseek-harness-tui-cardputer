#!/usr/bin/env python3
"""run_scenarios.py -- automated UI/behaviour checks for feature work.

Each directory under `scenarios/` holds:
  config.yml    ~/.dst/config.yml body (the `api.base` line is injected)
  scenario.json screen_sim.py scenario (keys, waits, snapshots)
  script.json   mock_api.py scripted model responses

The runner starts the mock API on an ephemeral localhost port, runs dst in the
pty simulator, then asserts on the captured screens and on the request log.
Everything stays on 127.0.0.1.

    python3 run_scenarios.py               # all scenarios
    python3 run_scenarios.py goal compact  # just these
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

from run_demo import DEFAULT_BIN, MOCK, REPO, SIM, log, start_mock, stop_mock

ROOT = os.path.dirname(os.path.abspath(__file__))
SCEN_DIR = os.path.join(ROOT, "scenarios")


def chat_requests(log_path):
    out = []
    if not os.path.exists(log_path):
        return out
    for line in open(log_path, encoding="utf-8"):
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue
        body = rec.get("body") or {}
        if body.get("messages"):
            out.append(body)
    return out


def message_texts(requests):
    texts = []
    for body in requests:
        for m in body.get("messages") or []:
            c = m.get("content")
            if isinstance(c, str):
                texts.append(c)
    return texts


def read_screens(out_dir):
    screens = {}
    for name in os.listdir(out_dir):
        if not name.endswith(".txt"):
            continue
        stem = name[:-4]
        text = open(os.path.join(out_dir, name), encoding="utf-8").read()
        screens[stem] = text
        # Snapshot files are numbered (01_splash.txt); also index by the bare
        # scenario snapshot name so checks do not depend on ordering.
        base = stem.split("_", 1)[1] if stem[:1].isdigit() and "_" in stem else stem
        screens.setdefault(base, text)
    return screens


def run_scenario(name, binary, timeout=90):
    base = os.path.join(SCEN_DIR, name)
    scenario = os.path.join(base, "scenario.json")
    script = os.path.join(base, "script.json")
    work = tempfile.mkdtemp(prefix=f"dst-scen-{name}-")
    home = os.path.join(work, "home")
    out = os.path.join(work, "out")
    os.makedirs(os.path.join(home, ".dst"), exist_ok=True)
    os.makedirs(out, exist_ok=True)
    log_path = os.path.join(work, "requests.jsonl")
    env = dict(os.environ)

    proc, url, _state = start_mock(0, script, log_path, env)
    try:
        # url is like http://127.0.0.1:PORT/v1
        cfg = open(os.path.join(base, "config.yml"), encoding="utf-8").read()
        cfg = cfg.replace("  key: test", f"  base: {url}\n  key: test")
        cfg_path = os.path.join(home, ".dst", "config.yml")
        open(cfg_path, "w", encoding="utf-8").write(cfg)
        os.chmod(cfg_path, 0o600)
        cmd = [sys.executable, SIM, "--bin", binary, "--home", home,
               "--cwd", work, "--out", out, "--scenario", scenario,
               "--text-only"]
        r = subprocess.run(cmd, cwd=ROOT, env=env, capture_output=True,
                           text=True, timeout=timeout)
        if r.returncode != 0:
            raise AssertionError(f"simulator failed: {r.stderr[-800:]}")
    finally:
        stop_mock(proc)

    screens = read_screens(out)
    reqs = chat_requests(log_path)
    return screens, reqs, work


# Each check returns a list of (ok, message) tuples.
def check_clear(screens, reqs):
    checks = []
    final = screens.get("cleared", "")
    checks.append(("hold Tab to talk" in final, "screen reset to splash"))
    checks.append(("reasoning text must disappear" not in final,
                   "no reasoning text survives /clear"))
    checks.append(("thinking" not in final, "no thinking panel survives /clear"))
    checks.append((len(reqs) == 1, "exactly one chat request"))
    return checks


def check_goal(screens, reqs):
    screens_all = "\n".join(screens.values())
    checks = []
    checks.append(("\u25ce goal 1/1" in screens_all, "goal banner shows round counter"))
    checks.append(("\u27f3 goal round 1" in screens_all, "automatic goal round prompt shown"))
    checks.append(("Round one: goal driver worked." in screens_all, "goal round answer rendered"))
    checks.append((len(reqs) == 1, "round cap stops automatic continuation"))
    texts = "\n".join(message_texts(reqs))
    checks.append(("<goal_round>" in texts, "round prompt carries <goal_round>"))
    checks.append(("Time sampled while preparing turn" in texts, "time context injected"))
    return checks


def check_compact(screens, reqs):
    texts = message_texts(reqs)
    joined = "\n".join(texts)
    checks = []
    checks.append(("You are now acting as a compaction engine" in joined,
                   "summariser instruction sent"))
    checks.append(("<compacted-summary>" in joined,
                   "checkpoint replacement reached a later request"))
    checks.append(("Second answer after the checkpoint." in "\n".join(screens.values()),
                   "post-compaction turn answered"))
    checks.append((len(reqs) >= 3, "requests observed: summariser + both turns"))
    return checks


CHECKS = {"clear": check_clear, "goal": check_goal, "compact": check_compact}


def main(argv):
    names = argv or sorted(CHECKS)
    binary = os.environ.get("DST_BIN", DEFAULT_BIN)
    failures = 0
    for name in names:
        if name not in CHECKS:
            print(f"unknown scenario {name!r}; known: {', '.join(sorted(CHECKS))}")
            return 2
        log(f"scenario: {name}")
        t0 = time.time()
        try:
            screens, reqs, work = run_scenario(name, binary)
        except Exception as exc:  # noqa: BLE001 - report and continue
            print(f"[FAIL] {name}: {exc}")
            failures += 1
            continue
        checks = CHECKS[name](screens, reqs)
        ok = all(c[0] for c in checks)
        for passed, label in checks:
            print(f"  {'ok  ' if passed else 'FAIL'} {label}")
        print(f"[{'PASS' if ok else 'FAIL'}] {name} ({time.time() - t0:.1f}s)")
        shutil.rmtree(work, ignore_errors=True)
        if not ok:
            failures += 1
    if failures:
        print(f"{failures} scenario(s) failed")
        return 1
    print("all scenarios passed")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
