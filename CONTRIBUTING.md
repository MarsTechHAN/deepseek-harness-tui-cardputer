# Contributing

Thanks for looking at `dst`. It is a small codebase with a few hard rules, and
those rules are what make it reviewable.

## Ground rules

- **No third-party crates.** `Cargo.toml` has an empty `[dependencies]` section and
  stays that way. If something needs JSON, YAML, base64 or a hash, write the
  smallest correct version against `std` and test it.
- **No credentials in the tree.** Keys, tokens, QR payloads from a real account
  and private endpoints do not belong in code, tests, fixtures, screenshots or
  commit messages. Tests use `key: test` against `127.0.0.1`.
- **The panel is 53×12.** New UI text must fit, or wrap deliberately. The i18n
  tests enforce that every language has every key.
- **Every tool call settles.** A tool either returns a canonical value, a
  timeout or a reasoned failure — never nothing.

## Getting set up

```sh
git clone git@github.com:MarsTechHAN/deepseek-harness-tui-cardputer.git
cd deepseek-harness-tui-cardputer

cargo build            # host binary: target/debug/dst
cargo test             # 70 unit tests, no network
tools/deploy/deploy.sh --build-only   # optional: aarch64 cross build + ELF check
```

The device is never used as a build box: `tools/deploy/deploy.sh` cross-compiles
on the host and pushes the binary (see `README.md`).

## Trying it without the device

The screen simulator runs the real binary in a 53×12 pty and renders PNGs at the
panel's native resolution:

```sh
cd tools/sim
python3 run_demo.py          # scripted demo + contact sheet (no network, no key)
python3 run_scenarios.py     # feature scenarios, asserts on screens and request logs
python3 check_memory.py      # RSS over 20 turns
```

Screenshots in `docs/` are regenerated with `run_live.py` (a real endpoint) and
`run_demo.py`. If you change the UI, include the regenerated image in the PR.

## Making a change

1. Keep the diff focused; one concern per commit.
2. Add or extend a unit test for harness, tool, compaction, goal, config, skills
   or apps changes.
3. If the change is visible, add a scenario in `tools/sim/scenarios/<name>/` and
   assert on the captured screen *and* the request log.
4. Run `cargo test` (and `cargo build --release` if you touched the binary size or
   the release profile).
5. Commit messages: `area: what changed` — e.g. `sandbox: refuse writes under
   /boot`. Squash fixups before opening the PR.

## Reporting bugs

Please include:

- the device or host (Cardputer Zero, Linux version, `uname -a`),
- `dst --tools` output,
- the command or keystrokes that reproduced it,
- the relevant `~/.dst/logs/` file, with credentials removed.

Security issues go to the private channel described in `SECURITY.md`, not the
public tracker.
