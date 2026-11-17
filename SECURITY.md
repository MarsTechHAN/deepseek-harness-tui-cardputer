# Security policy

## What `dst` stores

| item | location | protection |
| --- | --- | --- |
| API key / DSH token | `~/.dst/config.yml` | file mode `600` |
| saved sessions | `~/.dst/sessions/` | plain JSON, user-owned |
| input history | `~/.dst/history` | mode `600`, `/key` never recorded |
| app and skill packages | `~/.dst/apps`, `~/.dst/skills` | user-owned |

Nothing is sent anywhere except the endpoint you configure. There is no
telemetry, no crash upload and no remote logging. HTTPS is enforced: a plain
`http://` endpoint is refused except for `127.0.0.1`.

## Tool execution

Every tool call passes the permission gate described in
`docs/ARCHITECTURE.md`. In the default `auto` mode, system changes are refused
outright and ambiguous calls are judged by a short model call. Creator-mode app
launches get ulimits, a timeout, a scrubbed environment and a working directory
inside the package.

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's private advisory flow
("Security" → "Report a vulnerability") on
`MarsTechHAN/deepseek-harness-tui-cardputer`, or email
`hansh-sz@hotmail.com` with `[dst security]` in the subject.

Include a description, the version/commit, and a minimal reproduction. You can
expect an acknowledgement within a few days and a fix or mitigation as soon as
the issue is understood.

## Supported versions

This is a personal project tracking `main`: fixes land on `main` and are
backported only if a release tag exists.
