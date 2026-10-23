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
4. As an operator, I want to run it on Linux or macOS, so that I can run it on
   the machine I have.
5. As an operator, I want what Linewright does available as commands, so that I
   and coding agents can drive and debug it.
6. As an operator, I want the server's log to say what it is doing and why, so
   that I can debug it without GitHub.
7. As an operator, I want to replace the binary without wasting the work in
   flight, so that an upgrade costs no agent sessions.

## Commands

The command is `lw`.

| command               | what it does                                                             |
| --------------------- | ------------------------------------------------------------------------ |
| `serve`               | poll until drained or stopped (see [Stopping](#stopping))                |
| `once`                | one poll cycle, then exit                                                |
| `drain`               | tell `serve` to drain (see [Stopping](#stopping))                        |
| `status`              | show what Linewright is doing (see [Monitoring](monitoring.md))          |
| `explain N`           | show where an item stands and why (see [Monitoring](monitoring.md))      |
| `runs [N]`            | list the runs (see [Monitoring](monitoring.md))                          |
| `logs N`              | show a run's transcript (see [Monitoring](monitoring.md))                |
| `automation run NAME` | run one automation now (see [Automations](automations.md))               |
| `capture`             | write a case from GitHub (see [Replay](replay.md))                       |
| `replay`              | run a case against a local store (see [Replay](replay.md))               |

- **The version.** `--version` and the server's first log line name the
  version and the commit it was built from.

## Stopping

`serve` ends in one of three ways, each more abrupt than the last: a drain,
which wastes no work; a stop, which keeps unfinished work; and an exit at
once.

| State    | Event                           | Next     | Effect                                                                              |
| -------- | ------------------------------- | -------- | ----------------------------------------------------------------------------------- |
| serving  | SIGQUIT or `drain`              | draining | claims no item and starts no run; exits once no run is in flight                    |
| serving  | SIGINT (Ctrl-C) or SIGTERM      | stopping | cancels the runs in flight once the action in progress is done; exits once they end |
| draining | SIGQUIT or `drain`              | draining | nothing more                                                                        |
| draining | SIGINT, SIGTERM or the deadline | stopping | as from serving                                                                     |
| stopping | SIGQUIT or `drain`              | stopping | nothing more                                                                        |
| stopping | SIGINT or SIGTERM               | exited   | at once                                                                             |

- **Draining.** All else `serve` does goes on until it exits, polls and
  deliveries included, and it waits for every run in flight however long,
  one waiting out the usage limit included.
- **Exit status.** 0 after a drain or a stop; 128 plus the signal's number
  after an exit at once.
- **`drain`.** `drain` asks the `serve` of the configured `repo` on the same
  host to drain, prints the runs it waits for and each as it ends, and
  returns once `serve` has exited; `--no-wait` returns once `serve` has taken
  the request. Ending `drain` itself leaves the drain in place, and a `drain`
  sent to a `serve` already draining or stopping says so and waits the same
  way. Its output and exit statuses are the monitoring commands'
  ([Monitoring](monitoring.md)).
- **`--timeout`.** `drain --timeout 2h` sets a deadline, after which the
  drain becomes a stop; `drain` still returns 0, saying so. Without one, a
  drain waits however long. The latest `drain` sets the deadline.

## Configuration

Every setting is a flag, an environment variable (`LINEWRIGHT_<FLAG>`) or a
key in a TOML config file (`--config`, default `linewright.toml` in the
working directory), with precedence flag, then environment, then file, then
default.

| setting                                      | default                                     |
| -------------------------------------------- | ------------------------------------------- |
| `repo` (`owner/name`)                        | required                                    |
| `poll-interval`                              | 60s                                         |
| `webhook-address`                            | `0.0.0.0:8080`                              |
| `ui-address`                                 | `127.0.0.1:8081`                            |
| `ui-hosts` (host names)                      | none: local names only                      |
| `spec-glob`                                  | `docs/**/*.md`                              |
| `spec-lookback`                              | 7d                                          |
| `max-runs`                                   | 4                                           |
| `work-dir`                                   | `linewright/` under the user's data directory |
| `model`                                      | one model for every stage                   |
| `briefs` (a directory)                       | the briefs Linewright ships                 |
| `session-env` (variable names)               | none                                        |
| `session-timeout`                            | 60m                                         |
| `result-retries`                             | 1                                           |
| `ci-grace`                                   | 5m                                          |
| `ci-timeout`                                 | 6h                                          |
| `ci-attempts`                                | 2                                           |
| `update-branch` (`rebase`, `merge`)          | from the repository's settings              |
| `auto-merge`                                 | off                                         |
| `merge-method` (`merge`, `squash`, `rebase`) | from the repository's settings              |
| `require-label`                              | none: every issue and pull request          |
| `log-format` (`text`, `json`)                | `text` on a terminal, `json` otherwise      |

- **Credentials.** They are the environment variables their tools already
  use, so an existing setup is picked up: `GITHUB_TOKEN` or `GH_TOKEN` as
  `gh` reads them, and `ANTHROPIC_API_KEY`. The webhook secret has no such
  variable and is `LINEWRIGHT_WEBHOOK_SECRET`, and the GitHub token agent
  sessions are given in place of Linewright's (see
  [Repository](repository.md)) is `LINEWRIGHT_SESSION_GITHUB_TOKEN`.
- **Times and durations.** Every time and duration given to Linewright, as a
  setting or as a command's argument, takes the same forms: a time in RFC
  3339 (`2026-10-14T12:00:00Z`) or a date, taken as midnight UTC
  (`2026-10-14`), and a duration as the settings' defaults write one, such as
  `90s`, `30m`, `6h` or `7d`.

## Sessions' environment

An agent session's environment holds only the variables of Linewright's
environment that this table or `session-env` names, and what Linewright sets
itself: the session's GitHub token, when it has one, as `GH_TOKEN` and
`GITHUB_TOKEN`, its own temporary directory as `TMPDIR`, and the harness's
settings that the rules under [Running](#running) need, such as the shell.

| variables                                                                | what for                                           |
| ------------------------------------------------------------------------ | -------------------------------------------------- |
| `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`                               | finding programs and the user's home               |
| `LANG`, `LANGUAGE`, `LC_*`, `TZ`                                         | locale and time zone                               |
| `TERM`                                                                   | the terminal tools format for                      |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`   | where tools keep their configuration and caches    |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY`, in either case     | reaching the network through the host's proxy      |
| `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`                   | trusting the host's certificate authorities        |
| `ANTHROPIC_*`, `CLAUDE_*`                                                | the harness's configuration and credentials        |

- **Patterns.** A name ending in `*` names every variable with that prefix.
- **The harness's namespaces.** Of the harness's variables, the table passes
  those in the namespaces it owns; a setting outside them, such as
  `DISABLE_TELEMETRY`, the operator names in `session-env` like any other
  variable.
- **Linewright's own win.** A variable Linewright sets itself replaces one of
  the same name the table or `session-env` passes.
- **Never passed.** `session-env` never passes Linewright's GitHub tokens or
  the webhook secret: a pattern skips them, and naming one is an error at
  startup.
- **Logged at startup.** At startup Linewright logs the names, never the
  values, of the variables in its environment that sessions are not given.

## Running

Linewright runs wherever `git`, `bash`, the harness and the project's
toolchain are on the path: directly on a machine, or in a container whose
image the operator builds with those in it.

- **TLS.** Linewright listens over plain HTTP; TLS, where wanted, is a
  reverse proxy's in front.
- **The shell.** Agents' commands run in `bash`, whatever shell the operator
  uses.
- **As installed.** A session never updates the harness, in the background
  or on an agent's command: every session runs the harness the operator
  installed.
- **The log.** The server logs each decision and action, readable for a
  person tailing it and structured (JSON lines) for machines, by the
  `log-format` setting. That includes, while it serves the web UI, its
  address and each request it refuses and why, and, with webhooks on, the
  address it listens on for them, each delivery and what it names, and each
  delivery refused and why.
- **Permissions.** Agents run without permission prompts; the container is
  the isolation when one is used.
- **Systems.** Unix-like systems: Linux and macOS.

## Out of scope

- One container per run, instead of agents as processes beside the manager.
- Keeping sessions from credentials in files Linewright's user can read, such
  as `~/.config/gh/hosts.yml` or `~/.aws/credentials`. A container, or a user
  of its own that holds no other credentials, is the isolation.
- How Linewright is released (`docs/releasing.md`).
- Calling off a drain; the restart it ends in takes the work up again.
- A setting that bounds every drain; the bound is each request's.
