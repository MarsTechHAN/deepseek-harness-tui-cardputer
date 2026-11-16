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
    try:
        import run_scenarios as _rs
        scenario_count = len(_rs.CHECKS)
    except Exception:
        scenario_count = "?"
    checks = [
        ("demo (screens + mock streaming)", ["run_demo.py"], False),
        (f"feature scenarios ({scenario_count})", ["run_scenarios.py"], False),
        ("login page i18n (en/zh/es/ja/de/it)", ["check_login_page_i18n.py"], quick),
        ("login cancel (phone page)", ["check_login_cancel.py"], quick),
        ("QR scanability (jsQR)", ["check_qr_scan.py"], quick),
        ("login screen layout (6 languages)", ["render_login_i18n.py"], quick),
        ("memory (20-turn long session)", ["check_memory.py"], quick),
    ]
    results = [(label, run(label, cmd, quick_skip=skip)) for label, cmd, skip in checks]
    failed = [label for label, ok in results if not ok]
    print("\n" + ("ALL CHECKS PASSED" if not failed else f"FAILED: {', '.join(failed)}"))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
