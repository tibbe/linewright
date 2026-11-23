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
8. As an operator, I want secrets kept out of what Linewright and its sessions
   produce, so that Linewright does not leak them.
9. As an operator, I want to see at a glance what Linewright is working on and
   what holds it back, such as the usage limit, so that I know whether it
   needs me.
10. As an agent, I want the monitoring commands' answers as compact text, and
    as JSON where I extract fields, so that I spend little context and can
    script against them.
11. As an operator away from Linewright's host, I want the same facts in the
    UI, so that I follow the pipeline from wherever I am.

## Commands

The command is `lw`.

| command               | what it does                                                             |
| --------------------- | ------------------------------------------------------------------------ |
| `serve`               | poll until drained or stopped (see [Stopping](#stopping))                |
| `once`                | one poll cycle, then exit                                                |
| `drain`               | tell `serve` to drain (see [Stopping](#stopping))                        |
| `environment`         | show the environment and its builds (see [`lw environment`](sessions.md#lw-environment)) |
| `environment rebuild` | build the environment now (see [Environment](sessions.md#environment))   |
| `status`              | show what Linewright is doing (see [`lw status`](#lw-status))            |
| `explain N`           | show where an item stands and why (see [`lw explain`](board.md#lw-explain)) |
| `runs [N]`            | list the runs (see [`lw runs`](runs.md#lw-runs))                         |
| `logs N`              | show a run's transcript (see [`lw logs`](runs.md#lw-logs))               |
| `export ID DIR`       | write run ID as plain files (see [`lw export`](runs.md#lw-export))       |
| `automation run NAME` | run one automation now (see [Automations](automations.md))               |
| `capture ID`          | capture run ID as a task in a benchmark (see [Benchmarks](benchmarks.md))    |
| `trial TASK`         | run a task's stage on it again (see [Benchmarks](benchmarks.md))         |
| `score TASK DIR`       | score the output of a trial (see [Benchmarks](benchmarks.md))           |
| `benchmarks`, `benchmark`, `benchmark-run` | work with benchmarks and benchmark runs (see [Benchmarks](benchmarks.md#benchmarks-and-benchmark-runs-on-the-command-line)) |

- **The version.** `--version` names the version and the commit it was
  built from.
- **Conventions.** Every command follows the conventions of Unix command-line
  tools, such as its answer going to standard output and all else to
  standard error.
- **Errors outside a log.** A command that keeps no log, or fails before its
  log begins, says why in one line, `lw: <why>`.

## Monitoring commands

The monitoring commands let the operator, the maintainer or an agent see what
a running Linewright is doing and why, as `serve` itself sees it. They talk to
the `serve` of the configured `repo` on the same host.

| command         | what it shows                                   | where                                          |
| --------------- | ----------------------------------------------- | ---------------------------------------------- |
| `status`        | what Linewright is doing and what holds it back | [`lw status`](#lw-status)                      |
| `explain N`     | where issue or pull request #N stands and why   | [`lw explain`](board.md#lw-explain)            |
| `runs [N]`      | the runs, all or #N's                           | [`lw runs`](runs.md#lw-runs)                   |
| `logs N`        | the transcript of #N's run                      | [`lw logs`](runs.md#lw-logs)                   |
| `environment`   | the environment and its builds' output          | [`lw environment`](sessions.md#lw-environment) |
| `export ID DIR` | run ID, written into DIR as plain files         | [`lw export`](runs.md#lw-export)               |

- **Without `serve`.** `status` and `explain` need it running; `runs`,
  `logs`, `environment` and `export` work either way.
- **Access.** File permissions: only `serve`'s user can open the socket.
- **The socket's path.** Derived from the repository, in a short per-user
  directory such as `$XDG_RUNTIME_DIR`.
- **Requests answered.** Only those whose `Host` is local: `localhost` or a
  loopback address, ignoring case, port and a trailing dot.

## `lw status`

`status` shows what Linewright is doing and what holds it back, in this
order:

1. Linewright itself: the facts in the table below.
2. Each run in flight: its run ID, item, stage and how long it has run.
3. Each other open managed item: where it stands and, in one line, what
   Linewright is waiting for on it.
4. The automations, as [Automations](automations.md) describes.

| Fact about Linewright                          | Shown                  |
| ---------------------------------------------- | ---------------------- |
| since when `serve` has run                     | always                 |
| the last poll and the next                     | always                 |
| the polls failed since the last that succeeded, with the last error | while there are any |
| the usage limit and when it resets             | while one is in force  |
| a file of the repository's Linewright cannot use, and its error ([Linewright's files](repository.md#linewrights-files)) | while there is one |
| the drain, since when, and its deadline if set | while one is under way |
| the environment: the commit it was built from and when, a build in progress and since when, and the latest build if it failed, with when ([Environment](sessions.md#environment)) | with a setup command |
| the runs in flight out of `max-runs`           | always                 |
| the trials and scorers of benchmark runs in flight out of `max-trials` | while there are any |
| the time of the last webhook delivery accepted | with webhooks on       |
| the webhook deliveries refused since `serve` started | with webhooks on |

## Output

The commands print text whether or not their output is a terminal: one fact
per line as `label: value`, items as `#N`, runs by run ID, and colour only on
a terminal.

- **Cut lists.** A list cut to a limit ends with a line saying how many
  entries it left out and how to show them.
- **`--json`.** It prints the same answer as JSON; `logs --json` prints one
  JSON object per transcript entry and line, two for an entry that runs when
  followed. Each of a tool call's objects carries the ID the agent gives the
  call, and a sub-agent's objects carry the ID of the call that started it.
- **Exit status.** It is:

  | status | meaning                                      |
  | ------ | -------------------------------------------- |
  | 0      | success                                      |
  | 1      | any other error                              |
  | 2      | the command line is wrong                    |
  | 3      | `serve` is not running for the repository    |
  | 4      | no such item or run                          |

## Stopping

`serve` ends in one of three ways, each more abrupt than the last: a drain,
which wastes no work; a stop, which keeps unfinished work; and an exit at
once.

| State    | Event                                    | Next     | Effect                                                                              |
| -------- | ---------------------------------------- | -------- | ----------------------------------------------------------------------------------- |
| serving  | SIGTERM, SIGINT (Ctrl-C) or `drain`      | draining | holds runs and claims as under [Holds](runs.md#holds); exits once no run is in flight |
| serving  | SIGQUIT                                  | stopping | cancels the runs in flight once the action in progress is done ([Runs](runs.md)); exits once they end |
| draining | `drain`                                  | draining | nothing more                                                                        |
| draining | SIGTERM, SIGINT, SIGQUIT or the deadline | stopping | as from serving                                                                     |
| stopping | `drain`                                  | stopping | nothing more                                                                        |
| stopping | SIGTERM, SIGINT or SIGQUIT               | exited   | at once                                                                             |

- **Logging a signal.** `serve` logs the way a signal takes and that the
  next signal takes the next one.
- **Exiting.** Once its runs have ended, Linewright waits however long for
  what is left before it exits, such as the removal of their clones
  ([Stages](runs.md#stages)).
- **Logging the waits.** Linewright logs each thing it waits for before it
  exits as the wait begins, and how long it took as it ends.
- **A signal under any command but `serve`.** It ends the command at once,
  cutting its runs short.
- **Draining.** All else `serve` does goes on until it exits, polls and
  deliveries included, and it waits for every run in flight however long,
  one waiting out the usage limit included.
- **Exit status.** 0 after a drain or a stop; 128 plus the signal's number
  after an exit at once.
- **`drain`.** `drain` asks the `serve` of the configured `repo` on the same
  host to drain, prints the runs it waits for and each as it ends, then each
  thing `serve` waits for before it exits and each as it ends, and returns
  once `serve` has exited; `--no-wait` returns once `serve` has taken
  the request. Ending `drain` itself leaves the drain in place, and a `drain`
  sent to a `serve` already draining or stopping says so and waits the same
  way. Its output and exit statuses are the monitoring commands'
  ([Output](#output)).
- **`--timeout`.** `drain --timeout 2h` sets a deadline, after which the
  drain becomes a stop; `drain` still returns 0, saying so. Without one, a
  drain waits however long. The latest `drain` sets the deadline.

## Configuration

Every setting is a key in the operator's TOML config file (`--config`, default
`linewright.toml` in the working directory); some are also keys in the
repository's file ([The repository's file](#the-repositorys-file)). Each place a
setting is set is a layer, highest first: flag, environment variable, the
repository's file, the operator's file, default. In a trial, a configuration
overrides them all ([Configurations](benchmarks.md#configurations)).

| setting                                      | default                                     |
| -------------------------------------------- | ------------------------------------------- |
| `repo` (`owner/name`)                        | required, but not for a command that reads a task |
| `poll-interval`                              | 60s                                         |
| `webhook-address`                            | `0.0.0.0:8080` with the webhook secret set; none without it |
| `ui-address`                                 | `127.0.0.1:8081`                            |
| `public-url` (the URL people reach the UI at) | none: no UI                                |
| `google-client-id`                           | none: no sign-in with Google                |
| `google-signup-domains` (domains)            | none: no identity meets the rule            |
| `github-client-id`                           | none: no sign-in with GitHub                |
| `github-signup-orgs` (organizations, `org/team` for a team) | none: no identity meets the rule |
| `manager` (`google:EMAIL`, `github:LOGIN`)   | none                                        |
| `spec-glob`                                  | `docs/**/*.md`                              |
| `spec-lookback`                              | 7d                                          |
| `done-window`                                | 7d                                          |
| `max-runs`                                   | 4                                           |
| `max-trials`                                | 1                                           |
| `work-dir`                                   | `linewright/` under the user's data directory |
| `model`                                      | required, for a command that starts a session |
| `effort` (`low`, `medium`, `high`, `xhigh`, `max`) | medium                                |
| `briefs` (the operator's layer, [Briefs](#briefs)) | none |
| `session-env` (variables' values)            | none                                        |
| `session-pass-env` (variable names)          | none                                        |
| `secrets` (variable names)                   | none                                        |
| `session-timeout`                            | 6h                                          |
| `setup.command` ([Environment](sessions.md#environment)) | none                            |
| `setup.max-age`                              | 24h                                         |
| `setup.timeout`                              | 60m                                         |
| `maintenance` (a command, [Maintenance](runs.md#maintenance)) | none                       |
| `result-retries`                             | 1                                           |
| `ci-grace`                                   | 5m                                          |
| `ci-timeout`                                 | 6h                                          |
| `ci-attempts`                                | 2                                           |
| `merge-method` (`merge`, `squash`, `rebase`) | from the repository's settings              |
| `auto-merge`                                 | off                                         |
| `require-label`                              | none: every issue and pull request          |
| `log-format` (`text`, `json`)                | `text` on a terminal, `json` otherwise      |

- **`repo` and a task.** A command that reads a task, such as `trial`, reads
  the same settings as every other command. It works on the repository of the
  task ([Trials](benchmarks.md#trials)). It needs no `repo`, and it
  ignores a `repo` that is set.
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
- **A table of settings.** A setting whose name holds a dot, such as
  `setup.command`, is a key in a table of the config file, `[setup]` with
  `command` in it. Its flag and environment variable name it with the dot as
  a `-`: `--setup-command` and `LINEWRIGHT_SETUP_COMMAND`.
- **A stage's table.** Its entries are settings, so it is a key only.
- **A wrong value.** A setting whose value is not one it takes, such as an
  `effort` outside its levels or an unknown key in a stage's table, is an
  error: at startup in the operator's layers
  ([Startup](repository.md#startup)), and a file with an error in the
  repository's ([Linewright's files](repository.md#linewrights-files)).
- **A feature's settings.** A setting that only one feature uses is an error
  at startup when it is not at its default while that feature is off
  ([Startup](repository.md#startup)):
  - `webhook-address` without the webhook secret;
  - `ui-address` without `public-url`;
  - an identity provider's settings, or `manager`, without `public-url`;
  - a provider's sign-up rule or client secret without its client ID;
  - `manager` naming a provider without a client ID.
- **Settings a feature needs.** A feature that is on without a setting it
  needs is an error at startup:
  - `public-url` with `ui-address` empty;
  - `public-url` without an identity provider's client ID;
  - a provider's client ID without its client secret.
- **Model and effort.** The operator's layers set `model` for the whole
  pipeline. A session's model and effort win over what the session's
  environment variables and the harness's own settings say.
  Linewright sets no model or effort for a session's sub-agents.
- **Credentials come from environment variables only.** They are the environment
  variables their tools already use, so an existing setup is picked up:
  `GITHUB_TOKEN` or `GH_TOKEN` as `gh` reads them, and `ANTHROPIC_API_KEY`. The
  webhook secret has no such variable and is `LINEWRIGHT_WEBHOOK_SECRET`, and
  the GitHub token agent sessions are given in place of Linewright's
  ([What a session has](sessions.md#what-a-session-has)) is
  `LINEWRIGHT_SESSION_GITHUB_TOKEN`. The identity providers' client secrets
  ([Signing in](access.md#signing-in)) are `LINEWRIGHT_GOOGLE_CLIENT_SECRET`
  and `LINEWRIGHT_GITHUB_CLIENT_SECRET`.
- **Linewright's GitHub token.** A command that runs sessions needs
  Linewright's GitHub token. Without that token, the command exits with an
  error at startup.
- **Times and durations.** Every time and duration given to Linewright, as a
  setting or as a command's argument, takes the same forms: a time in RFC
  3339 (`2026-10-14T12:00:00Z`) or a date, taken as midnight UTC
  (`2026-10-14`), and a duration as the settings' defaults write one, such as
  `90s`, `30m`, `6h` or `7d`.

## Stage settings

A stage's sessions take the session settings, `model`, `effort`,
`session-timeout`, `session-env` and `session-pass-env`, from the
whole pipeline and from the stage's own tables, which are layers above every
whole-pipeline layer ([Configuration](#configuration)).

- **The tables.** A stage's settings are a table in a config file,
  `[stage.NAME]` with the stage's name under [Runs](runs.md), such as
  `[stage.implement]`, with the session settings as its keys.
- **The scorer.** The session of the scorer takes its settings as the sessions
  of a stage do, from the table `[stage.scorer]`
  ([Scoring](benchmarks.md#scoring)).

## The repository's file

The repository configures the work its agents do in `.linewright/config.toml`
on its default branch, the same TOML as the operator's file.

- **Its keys.** The session settings ([Stage settings](#stage-settings))
  except `session-env` and `session-pass-env`, for the whole pipeline and in
  stage tables, and the `setup` table and `maintenance`, for the whole
  pipeline. Any other key is an error: the rest of the settings are the
  operator's.
- **When it is read, and a file with an error.** As for every file of
  Linewright's in the repository
  ([Linewright's files](repository.md#linewrights-files)).

## Briefs

A session's brief comes in three layers, each a directory laid out as
Linewright's own `briefs/`: the repository's `.linewright/briefs/`, the
operator's `briefs` directory and Linewright's own. Each file is taken from the
first layer that has it. In a trial, a configuration overrides the three
([Configurations](benchmarks.md#configurations)).

- **What a layer may hold.** A layer may hold any file of Linewright's own
  briefs, but for the result schemas in `results/`
  ([Contract](runs.md#contract)): the prompts in `prompts/`, the wording that
  Linewright fills into them in `parts/` and `outputs/`, and the plugin with
  its skills in `plugin/`.
- **Placeholders.** A file's placeholders are fixed: Linewright refuses a file
  that lacks one or holds one that it doesn't know.
- **The result contract.** The result's schema decides which result
  Linewright accepts, whatever a layer's wording of the result contract says.
- **A file that Linewright cannot use.** A file that a layer may not hold, or
  that Linewright refuses, hidden files aside, is an error: at startup in the
  operator's layer ([Startup](repository.md#startup)), and a file with an
  error in the repository's
  ([Linewright's files](repository.md#linewrights-files)).
- **Logged at startup.** Linewright logs which files the operator's layer
  provides.

## Secrets

Linewright keeps secrets out of what it and its sessions produce.

- **Secrets are redacted.** Linewright replaces each secret with `[redacted]`
  in what it and its sessions produce, before it keeps, logs or posts this
  output. It never changes a session's commits or attachments: it refuses to
  push or upload one that holds a secret ([Runs](runs.md)). Redaction is best
  effort. A secret of 1 to 5 characters is neither redacted nor refused,
  which Linewright warns of at startup, naming its variable; an empty one is
  ignored.
- **A secret** is a credential Linewright reads
  ([Configuration](#configuration)) or passes to a session
  ([Environment variables](sessions.md#environment-variables)), or the value of
  a variable `secrets` names.
- **The `secrets` setting.** `secrets` names the variables whose values are
  secrets; a name ending in `*` names every variable with that prefix. A name
  without `*` whose variable is unset or empty is an error at startup.

## Running

Linewright runs wherever `git`, `bash`, the harness and the project's
toolchain are on the path: directly on a machine, or in a container whose
image the operator builds with those in it.

- **The base image.** A published base image (`lw`, the harness, and the
  tools coding agents expect, such as `git`, `gh`, `jq`, `python3` and
  `ripgrep`) is the starting point for the operator's image.
- **TLS.** Linewright listens over plain HTTP; TLS, where wanted, is a
  reverse proxy's in front.
- **Certificate authorities.** Linewright trusts the host's certificate
  authorities, and only those.
- **Behind a proxy.** Event streams ask a reverse proxy in front to pass each
  event on as it is sent, unchanged, and to keep no copy, and send something
  at least every 15 seconds, so that the proxy never closes one as idle.
- **Systems.** Unix-like systems: Linux and macOS.
- **The work directory.** It is on a local filesystem
  ([Storage](architecture.md#storage)). As it starts, Linewright warns when the
  work directory is on a network filesystem.
- **git.** Linewright finds, once, the git binary that the `git` on its path
  runs, and runs that binary directly from then on.

## Serving the UI

`serve` serves the UI ([Shell](shell.md)) on `ui-address`, a host and port,
while `public-url` is set. People reach the UI at `public-url`.

- **The public URL.** It is HTTPS, or HTTP to a loopback address.
- **Access.** The people that Linewright signs in, each with what their role
  allows ([Access](access.md)).
- **Requests answered.** Only those whose `Host` is the public URL's host,
  compared ignoring case, port and a trailing dot. The others get 421
  Misdirected Request.

## Screens

| Screen | Reached from                                                                                      | Purpose                               |
| ------ | ------------------------------------------------------------------------------------------------- | ------------------------------------- |
| Server | the navigation ([Shell](shell.md)), and the notice of a hold or problem it tells more of | see the facts about Linewright itself |

### Server

- **Layout.** The facts
  about Linewright itself ([Facts](design_system.md#facts)), as a list of
  labels, each with its value beside it.
- **The facts.** Server shows each fact about Linewright that `status` shows,
  when `status` shows it ([`lw status`](#lw-status)). It shows the environment
  with its builds, as `environment` shows them
  ([`lw environment`](sessions.md#lw-environment)).
- **Builds.** A build of the environment shows as a transcript's maintenance
  command does, and opens the same way ([Run](runs.md#run)); a build in progress
  is followed.
- **The environment's status.** The environment shows as the status of the
  first row below that holds ([Status](design_system.md#status)).

  | The environment                 | Shows as  |
  | ------------------------------- | --------- |
  | a build runs                    | in flight |
  | its latest build failed         | failed    |
  | there is no environment yet     | paused    |
  | otherwise                       | done      |

## The log

A command that does work itself logs each decision and action; a command
that answers a question, or asks `serve` to act, logs nothing.

- **Format.** The log is readable for a person tailing it and structured
  (JSON lines) for machines, by the `log-format` setting.
- **What `serve` logs.** It includes:
  - while `serve` serves the UI, its address and each request it refuses and
    why;
  - why `serve` serves no UI, while it serves none;
  - what [Access](access.md) says it logs, such as who did each action;
  - with webhooks on, the address it listens on for them, each delivery and
    what it names, and each delivery refused and why.
- **The first line.** It names the version and the commit, as `--version`
  does.
- **The last line.** It is the exit, with the exit status. An exit at once
  logs no such line.

## Out of scope

- Acting from the UI on `serve` or on the work, but for capturing a run and
  for benchmarks and benchmark runs.
- Caps on what the repository's file may set, such as the models it may
  choose.
- The repository's file naming the variables a stage's sessions get, even
  from those the operator's `session-pass-env` names.
- A layer taking away what a lower one sets, such as a variable it passes.
- A value built from other variables, such as a `PATH` with a directory
  added.
- The settings the pipeline reads between runs, such as `auto-merge` or
  `max-runs`, in the repository's file, other than the `setup` table.
- How Linewright is released (`docs/releasing.md`).
- Calling off a drain; the restart it ends in takes the work up again.
- A setting that bounds every drain; the bound is each request's.
- A command that backs up the work directory.
- The monitoring commands from another host, but through a shell on, or a
  tunnel to, Linewright's host.
- A liveness check for a container healthcheck or a supervisor.
- A record of each decision Linewright makes on an item; the log holds them.
- Answers from `once` and `automation run` while they run.
- Direction, not the first version: commands that run single pieces of
  Linewright, so that a coding agent changing Linewright can verify its change.
