# Operation

Linewright is one self-contained server the operator runs where they choose,
directly on a machine or in a container for isolation.

For example, the operator runs Linewright on a server, in a container built
from Linewright's base image with the project's toolchain added.

## User Stories

1. As an operator, I want one binary configured by flags, environment variables
   or a config file, so that a deployment is reproducible.
2. As an operator, I want a one-shot mode, so that I can test a cycle by hand or
   run it from cron.
3. As an operator, I want to run it directly on a machine or inside a container,
   so that I choose the isolation.
4. As an operator, I want to run it on Linux in production and on macOS while
   developing.
5. As an operator, I want what Linewright does available as commands, so that I
   and coding agents can drive and debug it.
6. As an operator, I want the server's log to say what it is doing and why, so
   that I can debug it without GitHub.

## Decisions

- The command is `lw`, with these commands:

  | command | what it does |
  |---|---|
  | `serve` | poll until stopped |
  | `once` | one poll cycle, then exit |
  | `status` | show what Linewright is doing (see [Monitoring](monitoring.md)) |
  | `explain N` | show where an item stands and why (see [Monitoring](monitoring.md)) |
  | `runs [N]` | list the runs (see [Monitoring](monitoring.md)) |
  | `logs N` | show a run's transcript (see [Monitoring](monitoring.md)) |
  | `workflow run NAME` | run one workflow now (see [Workflows](workflows.md)) |
  | `capture` | write a case from GitHub (see [Replay](replay.md)) |
  | `replay` | run a case against a local store (see [Replay](replay.md)) |
- SIGINT (Ctrl-C) or SIGTERM stops `serve` once the action in progress is
  done, cancelling the runs in flight; a second one exits at once.
- Configuration: every setting is a flag, an environment variable
  (`LINEWRIGHT_<FLAG>`) or a key in a TOML config file (`--config`, default
  `linewright.toml` in the working directory), with precedence flag, then
  environment, then file, then default. Credentials are the environment
  variables their tools already use, so an existing setup is picked up:
  `GITHUB_TOKEN` or `GH_TOKEN` as `gh` reads them, and `ANTHROPIC_API_KEY`. The
  webhook secret has no such variable and is `LINEWRIGHT_WEBHOOK_SECRET`.

  | setting | default |
  |---|---|
  | `repo` (`owner/name`) | required |
  | `poll-interval` | 60s |
  | `webhook-address` | `0.0.0.0:8080` |
  | `spec-glob` | `docs/**/*.md` |
  | `spec-lookback` | 7d |
  | `max-runs` | 4 |
  | `work-dir` | `linewright/` under the user's data directory |
  | `model` | one model for every stage |
  | `briefs` (a directory) | the briefs Linewright ships |
  | `session-timeout` | 30m |
  | `result-retries` | 1 |
  | `ci-grace` | 5m |
  | `ci-timeout` | 6h |
  | `ci-attempts` | 2 |
  | `update-branch` (`rebase`, `merge`) | from the repository's settings |
  | `auto-merge` | off |
  | `merge-method` (`merge`, `squash`, `rebase`) | from the repository's settings |
  | `require-label` | none: every issue and pull request |
  | `log-format` (`text`, `json`) | `text` on a terminal, `json` otherwise |

- Every time and duration given to Linewright, as a setting or as a
  command's argument, takes the same forms: a time in RFC 3339
  (`2026-10-14T12:00:00Z`) or a date, taken as midnight UTC (`2026-10-14`),
  and a duration as the settings' defaults write one, such as `90s`, `30m`,
  `6h` or `7d`.
- Linewright runs wherever `git`, `bash`, the agent runtime and the project's
  toolchain are on the path: directly on a machine, or in a container whose
  image the operator builds with those in it. Linewright publishes a base
  image for `linux/amd64` and `linux/arm64` with everything but the project's
  toolchain to build from. Agents' commands run in `bash`, whatever shell the
  operator uses.
- The server logs each decision and action, readable for a person tailing it
  and structured (JSON lines) for machines, by the `log-format` setting.
  With webhooks on, that includes the address it listens on, each delivery
  and what it names, and each delivery refused and why.
- `--version` and the server's first log line name the version and the
  commit it was built from.
- Agents run without permission prompts; the container is the isolation
  when one is used.
- Unix-like systems: Linux is the deployment target, macOS a development
  host.

## Out of scope

- One container per run, instead of agents as processes beside the manager.
- Base images for architectures other than `linux/amd64` and `linux/arm64`.
