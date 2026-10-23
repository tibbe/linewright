# Monitoring

Monitoring lets the operator, the maintainer or an agent see what a running
Linewright is doing and why, as `serve` itself sees it.

For example, the operator wonders why #42 has not moved in an hour.
`lw explain 42` shows a rework run on it that has gone for 50 minutes,
and `lw logs 42 --follow` shows the agent waiting on a test that hangs.

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
5. As an agent, I want the same answers as compact text, and as JSON where
   I extract fields, so that I spend little context and can script against
   them.
6. As an operator away from Linewright's host, I want the same facts in a
   browser, so that I follow the pipeline from wherever I am.
7. As a maintainer, I want to see where my spec change or issue stands and
   what it waits for, as soon as it changes, so that I do not wait on GitHub
   for news.
8. As a maintainer, I want to read agents' transcripts, so that I can tune the
   repository's skills and instructions.

## Commands

The monitoring commands talk to the `serve` of the configured `repo` on the
same host.

| command     | what it shows                                     |
| ----------- | ------------------------------------------------- |
| `status`    | what Linewright is doing and what holds it back   |
| `explain N` | where issue or pull request #N stands and why     |
| `runs [N]`  | the runs, all or #N's                             |
| `logs N`    | the transcript of #N's run                        |

- **Without `serve`.** `status` and `explain` need it running; `runs` and
  `logs` work either way.

## status

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
| the drain, since when, and its deadline if set | while one is under way |
| the runs in flight out of `max-runs`           | always                 |
| the time of the last webhook delivery accepted | with webhooks on       |
| the webhook deliveries refused since `serve` started | with webhooks on |

## explain

`explain N` brings together what Linewright knows about #N, and points to
`runs` and `logs` for the detail.

- **What it shows.** For issue or pull request #N:
  - its parents and its children in the pipeline, such as the issues a pull
    request closes, or the pull requests that close a ticket;
  - where it is in the pipeline, in the states of the issue lifecycle under
    [Repository](repository.md), whether it is stalled, and when `serve`
    last read it;
  - each thing Linewright waits for before it acts on #N next, naming what
    ends the wait, such as "the usage limit, until 14:30 UTC" or "the drain,
    until `serve` restarts";
  - its run in flight;
  - its latest three past runs, as `runs N` lists them.
- **An item Linewright leaves.** For an item that is not managed,
  `serve` reads #N from GitHub and `explain` says why Linewright leaves it,
  naming the fact or setting that keeps it out.

## runs

`runs` lists every run, newest first, including runs that started no session:
its run ID, item, stage, start, duration and outcome.

- **One item.** `runs N` lists #N's.
- **Narrowing.** `--stage`, `--since` and `--limit` narrow the list.
  `--since` keeps the runs that started at or after a time, or a duration
  ago.
- **Outcome.** A run's outcome is one of those under [Runs](runs.md), by its
  name, and in JSON by its name in JSON.
- **Duration.** A run's duration is how long it ran, or for a run in flight
  how long it has run so far. An interrupted run has none: in JSON,
  `duration_seconds` is null.
- **Run ID.** A run ID is `<item>/<stage>-<n>`, the `n`th run of that stage
  on that item, such as `43/rework-2`, or `<automation>/<n>` for an automation's
  runs, with the stages under [Runs](runs.md).

## logs

`logs N` shows the transcript of #N's run in flight, or else of its latest run
that started a session.

- **Another run.** `--run ID` picks another run.
- **As the agent shows it.** `logs` shows a session as the agent itself shows
  it, as far as text that only ever grows allows, and `logs --json` gives
  what the agent gives about it, following the agent's own JSON output. The
  rules below settle where that leaves a choice.
- **Sessions and turns.** The transcript shows each of the run's sessions in
  order, and marks each turn after the first with the follow-up that started
  it.
- **Lines.** Each line starts with the time since its session started, as
  h:mm:ss with the hours left out under an hour, such as `2:10` or
  `1:02:10`, not as a duration is shown, so that the lines align. The
  agent's messages show in full. Each tool call shows on one line: the tool,
  its main argument and a short account of its result, such as the lines
  read, the exit status or, for a sub-agent, the count of its tool calls and
  how long it ran, or `running` while the call has not returned. A
  sub-agent's work shows indented under the call that started it.
- **`--verbose`.** It shows everything the transcript holds, including the
  agent's thinking, each tool's full input and result, and the brief and
  follow-ups.
- **`--follow`.** It shows the transcript so far, then each entry as it
  happens, and ends when the run ends. A tool call shows once it is made, and
  again with its account, at the time, once it returns or ends in the
  background.
- **No session.** For a run that started no session, `logs` says so.

## Output

The commands print text whether or not their output is a terminal: one fact
per line as `label: value`, items as `#N`, runs by run ID, and colour only on
a terminal.

- **Cut lists.** A list cut to a limit ends with a line saying how many
  entries it left out and how to show them.
- **`--json`.** It prints the same answer as JSON; `logs --json` prints one
  JSON object per transcript entry and line. Each of a tool call's objects
  carries the ID the agent gives the call, and a sub-agent's objects carry
  the ID of the call that started it.
- **Errors.** They go to standard error. The exit status is:

  | status | meaning                                      |
  | ------ | -------------------------------------------- |
  | 0      | success                                      |
  | 1      | any other error                              |
  | 2      | the command line is wrong                    |
  | 3      | `serve` is not running for the repository    |
  | 4      | no such item or run                          |

## Out of scope

- The monitoring commands from another host other than through a shell on,
  or a tunnel to, Linewright's.
- An MCP server.
- A liveness check for a container healthcheck or a supervisor.
- A record of each decision Linewright makes on an item; the log holds them.
- Answers from `once` and `automation run` while they run.
- Direction, not the first version: commands that run single pieces of
  Linewright, so that a coding agent changing Linewright can verify its change.
