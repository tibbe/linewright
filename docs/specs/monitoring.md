# Monitoring

Monitoring lets the operator, the maintainer or an agent see what a running
Linewright is doing and why, as `serve` itself sees it.

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
6. As an operator away from Linewright's host, I want the same answers in a
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
2. Each run in flight: its run ID, item, stage, how long it has run and its
   quiet time, how long since its agent last produced output.
3. Each other open managed item: where it stands and, in one line, what
   Linewright is waiting for on it.
4. The automations, as [Automations](automations.md) describes.

| Fact about Linewright                          | Shown                  |
| ---------------------------------------------- | ---------------------- |
| since when `serve` has run                     | always                 |
| the last poll and the next                     | always                 |
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
    [Repository](repository.md), and when `serve` last read it;
  - each thing Linewright waits for before it acts on #N next, naming what
    ends the wait, such as "the usage limit, until 14:30 UTC" or "the drain,
    until `serve` restarts";
  - its run in flight, with its quiet time;
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
- **Lines.** Each line starts with the time since its session started. The
  agent's messages show in full. Each tool call shows on one line: the tool,
  its main argument and a short account of its result, such as the lines
  read or the exit status, or `running` while the call has not returned. A
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
  the ID of the call that started it. A field, once printed, keeps its name
  and meaning; new fields may be added.
- **Errors.** They go to standard error. The exit status is:

  | status | meaning                                      |
  | ------ | -------------------------------------------- |
  | 0      | success                                      |
  | 1      | any other error                              |
  | 2      | the command line is wrong                    |
  | 3      | `serve` is not running for the repository    |
  | 4      | no such item or run                          |

## The web UI

`serve` serves a web UI that shows the commands' answers in a browser.

| Page        | Path           | Shows                                                       |
| ----------- | -------------- | ----------------------------------------------------------- |
| board       | `/`            | the managed items as cards ([The board](#the-board))        |
| item        | `/items/N`     | what `explain N` shows                                      |
| runs        | `/runs`        | what `runs` lists, with its narrowing as filters            |
| run         | `/runs/ID`     | what `logs` shows of run ID ([The run page](#the-run-page))  |
| automations | `/automations` | the automations ([The automations page](#the-automations-page)) |
| server      | `/server`      | the facts about Linewright itself, as `status` shows them   |

### Navigation

| Part        | Holds                                                     |
| ----------- | --------------------------------------------------------- |
| name        | the repository's name, linking to it on GitHub            |
| links       | the board, the runs, the automations and the server pages |
| status line | the runs in flight out of `max-runs`, and the last poll   |

### Holds and problems

| Hold or problem                          | Links to         |
| ---------------------------------------- | ---------------- |
| a usage limit in force                   | the server page  |
| a drain under way                        | the server page  |
| a webhook delivery refused since the last one accepted | the server page |
| an automation file that does not parse   | the automations page |
| an automation whose last run failed      | that run         |

### The board

The board shows every managed item as a card in one of six columns, in this
order:

| Column         | Holds a managed item that                                                                            |
| -------------- | ---------------------------------------------------------------------------------------------------- |
| Triage         | holds `needs-triage`                                                                                 |
| Needs a person | holds `needs-info` or `ready-for-human`                                                              |
| Ready          | holds `ready-for-agent` and is not claimed, or is a spec change not yet designed                     |
| Working        | is claimed for design or implementation, or has a run in flight that no other column shows           |
| In review      | has an open pull request that closes it, or is a pull request of Linewright's that closes no issue on the board |
| Done           | closed within the spec lookback window (`spec-lookback`)                                             |

- **Two columns.** An item that fits two columns is in the one further
  right.
- **One card per piece of work.** The card is the issue; a pull request, and
  a run on it, show on the card of each issue it closes.
- **A spec change.** Until it is designed, its card is its pull request;
  designed, it is its tracking issue's lane where one was cut, and otherwise
  its pull request, in Done, saying there was nothing to build.
- **Lanes.** A lane per tracking issue with cards on the board, headed by the
  tracking issue's number and title and its tickets closed of total, with its
  tickets as its cards; one more lane, headed "No tracking issue" and its
  count of cards, holds the items with no tracking issue.
- **Finished lanes.** A lane whose cards are all in Done is finished. It shows
  as its head alone, with when its last card closed, and opens on click. The
  finished lanes sit together under one fold that says how many there are
  and opens on click.
- **Order of lanes.** The lanes with a card in Needs a person come first,
  then the other unfinished lanes, then the finished lanes; unfinished lanes
  in number order with the lane of items with no tracking issue last, and
  finished lanes latest finished first.
- **Order in a column.** Ascending by number, within each lane.
- **No cap.** A column is never cut.
- **Needing a person.** The browser tab's title carries the count of cards in
  Needs a person.

### Cards

A card says what the item is and, in one line, what the pipeline is doing or
waiting for on it.

| Part             | Holds                                                                        |
| ---------------- | ---------------------------------------------------------------------------- |
| number and title | `#N` and the title, linking to the item page                                 |
| where it stands  | where it is in the pipeline, as `explain` words it                           |
| the line         | its run in flight, as the stage, how long it has run and its quiet time; or else each thing it waits for, as `explain` words it; or, in Done, how it ended: merged by which pull request, closed without merging, or closed |
| pull request     | its pull request, where it has one, with the pull request's CI state         |

### The run page

The run page shows a run's transcript as `logs` shows it.

- **Folding.** A tool call's line opens on click to the tool's full input and
  result; a call that failed starts open. A sub-agent's work starts folded to
  the line of the call that started it, with the count of its entries.
- **Verbose.** A switch, set once for the page, shows what `logs --verbose`
  shows, with everything open.
- **Following.** A run in flight is followed. The page stays at the bottom
  until the reader scrolls up, and then offers to jump to the latest entry.
- **Permalinks.** Each entry has a link of its own.

### The automations page

The automations page shows each automation file that does not parse, then a
row per automation.

| Part     | Holds                                                                   |
| -------- | ----------------------------------------------------------------------- |
| name     | the automation's name                                                   |
| schedule | when it runs, in words                                                  |
| last run | its last run, with its outcome, linking to the run; or, before its first, when Linewright first saw it |
| next run | when it runs next                                                       |

## Out of scope

- Acting from the UI on `serve` or on the work.
- A layout of the board for narrow screens.
- Monitoring from another host other than through a shell on, or a tunnel to,
  Linewright's; the direction is an authenticating proxy in front of the UI.
- An MCP server.
- A liveness check for a container healthcheck or a supervisor.
- A record of each decision Linewright makes on an item; the log holds them.
- Warnings about, or actions on, a run that stays quiet; the session timeout
  ends it.
- Answers from `once` and `automation run` while they run.
- Direction, not the first version: commands that run single pieces of
  Linewright, so that a coding agent changing Linewright can verify its change.
