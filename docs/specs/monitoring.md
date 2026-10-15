# Monitoring

Monitoring lets the operator or an agent see what a running Linewright is
doing and why, as `serve` itself sees it.

For example, the operator wonders why #42 has not moved in an hour.
`lw explain 42` shows a rework run on it whose agent has been quiet
for 25 minutes, and `lw logs 42 --follow` shows the agent waiting on a
test that hangs.

## User Stories

1. As an operator, I want to see at a glance what Linewright is working on and
   what holds it back, such as the usage limit, so that I know whether it needs
   me.
2. As an operator, I want to ask why an issue or pull request is not moving,
   including why Linewright is not working on it at all, so that I can unblock
   it.
3. As an operator, I want to follow what an agent is doing as it does it, so
   that I can tell an agent working hard from one that is stuck.
4. As an operator, I want to see what Linewright did on an item before, with
   each run's outcome and transcript, so that I understand how it got where it
   is.
5. As an agent, I want the same answers as compact text, and as stable JSON
   where I extract fields, so that I spend little context and can script
   against them.

## Decisions

### Commands

- The monitoring commands are:

  | command | what it shows |
  |---|---|
  | `status` | what Linewright is doing and what holds it back |
  | `explain N` | where issue or pull request #N stands and why |
  | `runs [N]` | the runs, all or #N's |
  | `logs N` | the transcript of #N's run |

- The commands talk to the `serve` of the configured `repo` on the same host.
  `status` and `explain` need it running; `runs` and `logs` work either way.
- Given a pull request Linewright opened for an issue, open or not, the
  commands show the issue's runs as the pull request's.

### status

- `status` shows, in order:
  - Linewright: since when `serve` has run, the last poll and the next, the
    usage limit while one is in force and when it resets, and the runs in
    flight out of `max-runs`. With webhooks on, also the time of the last
    delivery accepted and the number refused since `serve` started.
  - Each run in flight: its run ID, item, stage, how long it has run and its
    **quiet time**, how long since its agent last produced output.
  - Each other open item Linewright tracks: where it is in the pipeline and, in
    one line, what Linewright is waiting for on it.
  - The workflows, as [Workflows](workflows.md) describes.

### explain

`explain N` brings together what Linewright knows about #N, and points to
`runs` and `logs` for the detail.

- `explain N` shows, for issue or pull request #N:
  - its parent and its children in the pipeline, such as the issue a pull
    request is for, or a ticket's current pull request;
  - where it is in the pipeline, in the states of the issue lifecycle under
    [Repository](repository.md), and when `serve` last read it;
  - each thing Linewright waits for before it acts on #N next, with the
    particulars, such as "a free run: 4 of 4 in flight", "the usage limit,
    until 14:30 UTC", "CI on #43", "#40, which blocks it" or "the maintainer:
    labeled `ready-for-human`";
  - its run in flight, with its quiet time;
  - its latest three past runs, as `runs N` lists them.
- The parent of Linewright's pull request is the issue Linewright opened it
  for, whatever state the pull request is in and whatever issues it closes.
  `explain` names no other issue it closes as family. Each issue in review
  with it shows it as its current pull request.
- For an item Linewright does not act on, `serve` reads #N from GitHub and
  `explain` says why Linewright leaves it, naming the fact or setting that keeps
  it out.

### runs

- `runs` lists every run, newest first, including runs that started no session:
  its run ID, item, stage, start, duration and outcome. `runs N` lists #N's.
  `--stage`, `--since` and `--limit` narrow the list. `--since` keeps the runs
  that started at or after a time (`2026-10-14T12:00:00Z`), a date, taken as
  midnight UTC (`2026-10-14`), or a duration ago, written as the duration
  settings are (`2h`).
- A run's outcome, with its name in JSON, is one of:
  - **done** (`done`): it did its work, or found none left to do. A triage
    run is done whatever it decides, including `ready-for-human` or
    `needs-info`.
  - **handed back** (`handed-back`): it could not finish its work and handed
    the item to the maintainer: labeled `ready-for-human`, with a comment
    saying why.
  - **cancelled** (`cancelled`): its item stopped asking for the work, or
    `serve` was stopped.
  - **ended for the usage limit** (`usage-limit`): the agent runtime refused
    its session for the usage limit under `once` or `workflow run`, which do
    not wait it out.
  - **failed** (`failed`): it could not finish its work, and nothing on GitHub
    says so.
  - **in flight** (`in-flight`): it has not ended, including while it waits
    out the usage limit.
  - **interrupted** (`interrupted`): the process running it stopped before it
    ended, such as by a crash or a kill.
- A run's duration is how long it ran, or for a run in flight how long it has
  run so far. An interrupted run has none: in JSON, `duration_seconds` is null.
- A run ID is `<item>/<stage>-<n>`, the `n`th run of that stage on that item,
  such as `42/rework-2`, or `<workflow>/<n>` for a workflow's runs. The stages
  in IDs are `triage`, `tickets`, `implement`, `rework`, `fix-ci` and `update`.
- Runs are kept across restarts, with their transcripts.

### logs

- `logs N` shows the transcript of #N's run in flight, or else of its latest run
  that started a session. `--run ID` picks another run.
- `logs` shows a session as the agent itself shows it, as far as text that
  only ever grows allows, and `logs --json` gives what the agent gives about
  it, following the agent's own JSON output. The decisions below settle where
  that leaves a choice.
- The transcript shows each of the run's sessions in order, and marks each turn
  after the first with the follow-up that started it.
- Each line starts with the time since its session started. The agent's
  messages show in full. Each tool call shows on one line: the tool, its main
  argument and a short account of its result, such as the lines read or the
  exit status, or `running` while the call has not returned. A sub-agent's
  work shows indented under the call that started it.
- `--verbose` shows everything the transcript holds, including the agent's
  thinking, each tool's full input and result, and the brief and follow-ups.
- `--follow` shows the transcript so far, then each entry as it happens, and
  ends when the run ends. A tool call shows once it is made, and again with
  its account, at the time, once it returns or ends in the background.
- For a run that started no session, `logs` says so.

### Output

- The commands print text whether or not their output is a terminal: one fact
  per line as `label: value`, items as `#N`, runs by run ID, and colour only on
  a terminal.
- A list cut to a limit ends with a line saying how many entries it left out
  and how to show them.
- `--json` prints the same answer as JSON; `logs --json` prints one JSON object
  per transcript entry and line. Each of a tool call's objects carries the ID
  the agent gives the call, and a sub-agent's objects carry the ID of the call
  that started it. A field, once printed, keeps its name and meaning; new
  fields may be added.
- Errors go to standard error. The exit status is:

  | status | meaning |
  |---|---|
  | 0 | success |
  | 1 | any other error |
  | 2 | the command line is wrong |
  | 3 | `serve` is not running for the repository |
  | 4 | no such item or run |

## Out of scope

- A web UI.
- Monitoring from another host, other than through a shell on Linewright's.
- An MCP server.
- A liveness check for a container healthcheck or a supervisor.
- A record of each decision Linewright makes on an item; the log holds them.
- Warnings about, or actions on, a run that stays quiet; the session timeout
  ends it.
- Answers from `once` and `workflow run` while they run.
- Direction, not the first version: commands that run single pieces of
  Linewright, so that a coding agent changing Linewright can verify its change.
