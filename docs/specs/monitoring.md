# Monitoring

Monitoring lets the operator or an agent see what a running Linewright is
doing and why, as `serve` itself sees it.

For example, the operator wonders why #42 has not moved in an hour.
`linewright explain 42` shows a rework run on it whose agent has been quiet
for 25 minutes, and `linewright logs 42 --follow` shows the agent waiting on a
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

- `explain N` shows, for issue or pull request #N:
  - where it is in the pipeline, in the states of the issue lifecycle under
    [Repository](repository.md), and when `serve` last read it;
  - each thing Linewright waits for before it acts on #N next, with the
    particulars, such as "a free run: 4 of 4 in flight", "the usage limit,
    until 14:30 UTC", "CI on #43", "#40, which blocks it" or "the maintainer:
    labeled `ready-for-human`";
  - its run in flight, with its quiet time;
  - its past runs, as `runs N` lists them.
- For an item Linewright does not act on, `serve` reads #N from GitHub and
  `explain` says why Linewright leaves it, naming the fact or setting that keeps
  it out.

### runs

- `runs` lists every run, newest first, including runs that started no session:
  its run ID, item, stage, start, duration and outcome. `runs N` lists #N's.
  `--stage`, `--since` and `--limit` narrow the list.
- A run ID is `<item>/<stage>-<n>`, the `n`th run of that stage on that item,
  such as `42/rework-2`, or `<workflow>/<n>` for a workflow's runs. The stages
  in IDs are `triage`, `tickets`, `implement`, `rework`, `fix-ci` and `update`.
- Runs are kept across restarts, with their transcripts.

### logs

- `logs N` shows the transcript of #N's run in flight, or else of its latest run
  that started a session. `--run ID` picks another run.
- The transcript shows each of the run's sessions in order, and marks each turn
  after the first with the follow-up that started it.
- Each line starts with the time since its session started. The agent's
  messages show in full. Each tool call shows on one line: the tool, its main
  argument and a short account of its result, such as the lines read or the
  exit status. A sub-agent's work shows indented under the call that started
  it.
- `--verbose` shows everything the transcript holds, including the agent's
  thinking, each tool's full input and result, and the brief and follow-ups.
- `--follow` shows the transcript so far, then each entry as it happens, and
  ends when the run ends.
- For a run that started no session, `logs` says so.

### Output

- The commands print text whether or not their output is a terminal: one fact
  per line as `label: value`, items as `#N`, runs by run ID, and colour only on
  a terminal.
- A list cut to a limit ends with a line saying how many entries it left out
  and how to show them.
- `--json` prints the same answer as JSON; `logs --json` prints one JSON object
  per transcript entry and line. A field, once printed, keeps its name and
  meaning; new fields may be added.
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
