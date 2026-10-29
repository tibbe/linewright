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

| State    | Event                                    | Next     | Effect                                                                              |
| -------- | ---------------------------------------- | -------- | ----------------------------------------------------------------------------------- |
| serving  | SIGTERM, SIGINT (Ctrl-C) or `drain`      | draining | claims no item and starts no run; exits once no run is in flight                    |
| serving  | SIGQUIT                                  | stopping | cancels the runs in flight once the action in progress is done ([Runs](runs.md)); exits once they end |
| draining | `drain`                                  | draining | nothing more                                                                        |
| draining | SIGTERM, SIGINT, SIGQUIT or the deadline | stopping | as from serving                                                                     |
| stopping | `drain`                                  | stopping | nothing more                                                                        |
| stopping | SIGTERM, SIGINT or SIGQUIT               | exited   | at once                                                                             |

- **Logging a signal.** `serve` logs the way a signal takes and that the
  next signal takes the next one.
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

Every setting is a key in the operator's TOML config file (`--config`, default
`linewright.toml` in the working directory); some are also keys in the
repository's file ([The repository's file](#the-repositorys-file)).
Each place a setting is set is a layer, highest first: flag, environment, the
repository's file, the operator's file, default.

| setting                                      | default                                     |
| -------------------------------------------- | ------------------------------------------- |
| `repo` (`owner/name`)                        | required                                    |
| `poll-interval`                              | 60s                                         |
| `webhook-address`                            | `0.0.0.0:8080` with the webhook secret set; none without it |
| `ui-address`                                 | `127.0.0.1:8081`                            |
| `ui-hosts` (host names)                      | none: local names only                      |
| `spec-glob`                                  | `docs/**/*.md`                              |
| `spec-lookback`                              | 7d                                          |
| `done-window`                                | 7d                                          |
| `max-runs`                                   | 4                                           |
| `work-dir`                                   | `linewright/` under the user's data directory |
| `model`                                      | the harness's choice                        |
| `effort` (`low`, `medium`, `high`, `xhigh`, `max`) | the harness's choice                  |
| `prompts` (the operator's layer, [Prompts](#prompts)) | none                               |
| `session-env` (variables' values)            | none                                        |
| `session-pass-env` (variable names)          | none                                        |
| `secrets` (variable names)                   | none                                        |
| `session-timeout`                            | 60m                                         |
| `setup` (a command, [Setup](runs.md#setup))  | none                                        |
| `result-retries`                             | 1                                           |
| `ci-grace`                                   | 5m                                          |
| `ci-timeout`                                 | 6h                                          |
| `ci-attempts`                                | 2                                           |
| `update-branch` (`rebase`, `merge`)          | from the repository's settings              |
| `auto-merge`                                 | off                                         |
| `merge-method` (`merge`, `squash`, `rebase`) | from the repository's settings              |
| `require-label`                              | none: every issue and pull request          |
| `log-format` (`text`, `json`)                | `text` on a terminal, `json` otherwise      |

- **Combining layers.** A setting's value combines what every layer sets, by
  what the setting holds: a value is the highest layer's, a list holds the
  entries of every layer, and a table holds the names of every layer, each
  name's value combined as a value is.
- **A value or a list.** A setting that holds one is also a flag and an
  environment variable (`LINEWRIGHT_<FLAG>`).
- **A table of values.** A setting that holds one is also a flag, given once
  per entry and split at the first `=`, such as `--session-env NAME=value`,
  and has no environment variable. An entry without `=`, or a name given
  twice in one layer, is an error at startup.
- **A stage's table.** Its entries are settings, so it is a key only.
- **A wrong value.** A setting whose value is not one it takes, such as an
  `effort` outside its levels or an unknown key in a stage's table, is an
  error: at startup in the operator's layers
  ([Startup](repository.md#startup)), and a file with an error in the
  repository's ([Linewright's files](repository.md#linewrights-files)).
- **A feature's settings.** A setting that only one feature uses is an error
  at startup when it is not at its default while that feature is off
  ([Startup](repository.md#startup)): `webhook-address` without the webhook
  secret, `ui-hosts` with `ui-address` empty, `merge-method` without
  `auto-merge`.
- **Model and effort.** A model or effort set for a session wins over what
  the session's environment and the harness's own settings say.
- **Credentials.** They are the environment variables their tools already
  use, so an existing setup is picked up: `GITHUB_TOKEN` or `GH_TOKEN` as
  `gh` reads them, and `ANTHROPIC_API_KEY`. The webhook secret has no such
  variable and is `LINEWRIGHT_WEBHOOK_SECRET`, and the GitHub token agent
  sessions are given in place of Linewright's (see
  [Repository](repository.md)) is `LINEWRIGHT_SESSION_GITHUB_TOKEN`.
- **Secrets.** `secrets` names the variables whose values are secrets
  ([Architecture](architecture.md#capabilities-and-credentials)); a name
  ending in `*` names every variable with that prefix. A name without `*`
  whose variable is unset or empty is an error at startup.
- **Times and durations.** Every time and duration given to Linewright, as a
  setting or as a command's argument, takes the same forms: a time in RFC
  3339 (`2026-10-14T12:00:00Z`) or a date, taken as midnight UTC
  (`2026-10-14`), and a duration as the settings' defaults write one, such as
  `90s`, `30m`, `6h` or `7d`.

## Stage settings

A stage's sessions take the session settings, `model`, `effort`,
`session-timeout`, `setup`, `session-env` and `session-pass-env`, from the
whole pipeline and from the stage's own tables, which are layers above every
whole-pipeline layer ([Configuration](#configuration)).

- **The tables.** A stage's settings are a table in a config file,
  `[stage.NAME]` with the stage's name under [Runs](runs.md), such as
  `[stage.implement]`, with the session settings as its keys.

## The repository's file

The repository configures the work its agents do in `.linewright/config.toml`
on its default branch, the same TOML as the operator's file.

- **Its keys.** The session settings ([Stage settings](#stage-settings))
  except `session-env` and `session-pass-env`, for the whole pipeline and in
  stage tables. Any other key is an error: the rest of the settings are the
  operator's.
- **When it is read, and a file with an error.** As for every file of
  Linewright's in the repository
  ([Linewright's files](repository.md#linewrights-files)).

## Prompts

A session's prompts come in three layers, each a directory laid out as the
prompts Linewright ships: the repository's `.linewright/prompts/`, the
operator's `prompts` directory and Linewright's own, `briefs/prompts/`. Each
file is taken from the first layer that has it. Linewright's own layer holds
only what a layer may override: the stage prompts ([Stages](runs.md#stages))
and the follow-ups ([Contract](runs.md#contract)).

- **Placeholders.** A prompt's placeholders are fixed: Linewright refuses a
  prompt that lacks one or holds one it doesn't know.
- **The result contract.** A prompt's `{{output}}` is filled with
  Linewright's own wording of its result contract, so a prompt written over
  keeps the contract Linewright reads.
- **Other files.** A file in the operator's layer that is not one of
  Linewright's prompts, hidden files aside, is an error at startup
  ([Startup](repository.md#startup)).
- **Logged at startup.** Linewright logs which files the operator's layer
  provides.

## Sessions' environment

An agent session's environment holds only the variables of Linewright's
environment that this table or `session-pass-env` names, the values
`session-env` sets, and what Linewright sets itself: the session's GitHub
token, when it has one, as `GH_TOKEN` and `GITHUB_TOKEN`, its own temporary
directory as `TMPDIR`, and the harness's settings that the rules under
[Running](#running) need, such as the shell.

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
  `DISABLE_TELEMETRY`, the operator passes or sets like any other variable.
- **Which value wins.** A variable Linewright sets itself replaces one of the
  same name the table or `session-pass-env` passes, and a value `session-env`
  sets replaces a passed one.
- **Linewright's own.** `session-env` setting a variable Linewright may set
  itself is an error at startup.
- **Set values.** `session-env`'s values are strings, taken as written, such
  as a `$`, which refers to no variable, or an empty string, which sets the
  variable empty.
- **What an environment can't hold.** A name or value `session-env` sets that
  an environment can't hold is an error at startup, such as an empty name, an
  `=` in a name or a NUL byte in either.
- **Credentials passed.** The credentials the table passes are those its
  variables hold, where each variable's format puts them:

  | variables                                    | the credentials                                                                                                  |
  | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
  | those the harness reads a credential from    | the value                                                                                                        |
  | the proxy variables                          | the password in the URL, both as written and percent-decoded                                                     |
  | those the harness reads request headers from | each header's value, both whole and, when it opens with a scheme, without it, as in `Authorization: Bearer …` |
- **Never passed.** `session-pass-env` never passes Linewright's GitHub
  tokens or the webhook secret: a pattern skips them, and naming one is an
  error at startup.
- **Logged at startup.** At startup Linewright logs the names, never the
  values, of the variables in its environment that sessions are not given,
  and, for the whole pipeline and for each stage, the names `session-env`
  sets, a stage's own apart from those it takes from the whole pipeline's.

## Running

Linewright runs wherever `git`, `bash`, the harness and the project's
toolchain are on the path: directly on a machine, or in a container whose
image the operator builds with those in it.

- **TLS.** Linewright listens over plain HTTP; TLS, where wanted, is a
  reverse proxy's in front.
- **Certificate authorities.** Linewright trusts the host's certificate
  authorities, and only those.
- **Behind a proxy.** Event streams ask a reverse proxy in front to pass each
  event on as it is sent, unchanged, and to keep no copy, and send something
  at least every 15 seconds, so that the proxy never closes one as idle.
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
- Caps on what the repository's file may set, such as the models it may
  choose.
- The repository's file naming the variables a stage's sessions get, even
  from those the operator's `session-pass-env` names.
- A layer taking away what a lower one sets, such as a variable it passes or
  a host it allows.
- A value built from other variables, such as a `PATH` with a directory
  added.
- The settings the pipeline reads between runs, such as `auto-merge` or
  `max-runs`, in the repository's file.
- Layers of skills: the repository or the operator replacing Linewright's
  skills.
- Keeping sessions from, or redacting, credentials stored where Linewright's
  user can read them, such as `~/.config/gh/hosts.yml`, `~/.aws/credentials`
  or the harness's own login. A container, or a user of its own that holds no
  other credentials, is the isolation.
- How Linewright is released (`docs/releasing.md`).
- Calling off a drain; the restart it ends in takes the work up again.
- A setting that bounds every drain; the bound is each request's.
