#!/usr/bin/env python3
"""run_all.py -- run every offline simulator check in one command.

    python3 run_all.py            # build target/debug/dst first
    python3 run_all.py --quick    # skip the long memory/long-session checks

Exit status is non-zero if any check fails.
"""

from __future__ import annotations

import subprocess
import sys
import time

HERE = __import__("os").path.dirname(__import__("os").path.abspath(__file__))


def run(label, cmd, quick_skip=False):
    print(f"\n===== {label} =====", flush=True)
    if quick_skip:
        print("(skipped: --quick)")
        return True
    t0 = time.time()
    r = subprocess.run([sys.executable] + cmd, cwd=HERE)
    ok = r.returncode == 0
    print(f"===== {label}: {'PASS' if ok else 'FAIL'} ({time.time() - t0:.1f}s) =====")
    return ok


def main(argv):
    quick = "--quick" in argv
    results = [
        run("demo (screens + mock streaming)", ["run_demo.py"]),
        run("feature scenarios (26)", ["run_scenarios.py"]),
        run("memory (20-turn long session)", ["check_memory.py"], quick_skip=quick),
    ]
    failed = [label for label, ok in zip(["demo", "scenarios", "memory"], results) if not ok]
    print("\n" + ("ALL CHECKS PASSED" if not failed else f"FAILED: {', '.join(failed)}"))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
