# Design system

## Liveness

- A page shows `serve`'s state as it changes, without the reader reloading
  it.
- A page that loses `serve` keeps what it last showed, says since when, and
  reconnects on its own.
- What the reader opened or closed stays so through updates, until they
  open the page again.

## Words

- A page states what is so, not what to do about it; how to work with
  Linewright is the user guide's.

## Navigation

- Every page has the same navigation.
- A hold on Linewright's work, such as a usage limit, or a problem no item
  shows, is shown on every page while it lasts, linking to the page that
  tells more.
- Every item or pull request named on a page is named by its number, and
  its title where Linewright knows it, except in a card's line, which names
  it by its number alone, and links to its page and to GitHub.

## Time

- **Past times.** A past time is shown relative, in words and rounded down
  to whole minutes, hours or days, such as "5 minutes ago", keeping up as
  time passes, with the absolute time in the reader's time zone shown on
  hover. Under a minute it is "just now", and past 30 days it is the date,
  such as "on Sep 12", with the year when it is not this year, such as "on
  Sep 12, 2025".
- **Future times.** A future time is shown absolute, in the reader's time
  zone.
- **Durations.** A duration is shown in narrow units, to its two largest
  units that are not zero, such as "45s", "1m 43s", "3h 2m" or "1d 5m", and
  "0s" under a second, keeping up as time passes for a run in flight.
- **Waiting.** How long anything else has gone on is shown as when it
  began, a past time.

## Status

- An outcome or a state has an icon and a colour, shared by those that mean
  the same, and is never told by colour alone.

## Board

- As the reader scrolls, the column heads stay at the top, with the head of
  the lane they are in under them.
- A lane is a surface sunk into the page, and its cards are raised on it.
- A card's title is on at most two lines.

## Tables

- As the reader scrolls, a table's column heads stay at the top.
- A title in a table is on one line.

## Facts

- A page's facts about one thing are a list of labels, each with its value
  beside it.
- A label is words alone, with no icon or colour. A status among the facts
  is in the value: its icon and its words in its colour, with nothing behind
  them.

## Transcripts

- **Sessions and turns.** A run's transcripts show in the order of its
  sessions, each as the agent itself shows it where the rules below leave a
  choice, marking each turn after the first with the follow-up that started
  it.
- **Times.** Each entry starts with the time since its session started, as
  h:mm:ss with the hours left out under an hour, such as `2:10` or
  `1:02:10`, not as a duration is shown, so that the entries align.
- **Entries.** The agent's messages show in full. Each tool call shows on one
  line: the tool, its main argument and a short account of its result, such
  as the lines read, the exit status or, for a sub-agent, the count of its
  tool calls and how long it ran. A sub-agent's work shows under the call that
  started it.
- **Setup.** A session's first entry is its setup command
  ([Setup](runs.md#setup)): a line with the command, its exit status and how
  long it ran, then its output, which shows only in full or when the command
  failed. An empty one shows nothing.
- **Running.** A tool call and the setup command are entries that run: each
  shows `running` in place of its account until it ends.
- **In full.** A transcript in full shows everything it holds, including the
  agent's thinking, each tool's full input and result, and the brief and
  follow-ups.
- **Followed.** A followed transcript shows each entry as it happens. An entry
  that runs shows once it starts, and again with its account, at the time,
  once it ends: a tool call when it returns or ends in the background.

## Layout

- Content wider than the page scrolls sideways on its own, not the page with
  it.
- What a page shows on hover it also shows on keyboard focus, over the
  page, until the pointer leaves it or focus moves. Text cut to fit is shown
  whole this way.

## Type

- Secondary text, text that qualifies the text it sits beside, such as a
  card's line under its title, a fact's label or a time beside a transcript
  entry, is quieter than it.

## Colour scheme

- Light or dark follows the reader's preference.

## The web UI

`serve` serves a web UI for the maintainers, where they follow Linewright
and its work ([Monitoring](monitoring.md)), laid out for someone scanning a
page.

| Page        | Path           | Shows                                                       |
| ----------- | -------------- | ----------------------------------------------------------- |
| board       | `/`            | the managed items as cards ([The board](#the-board))        |
| item        | `/items/N`     | where #N stands, what it waits for, the items it relates to and its runs ([The item page](#the-item-page)) |
| runs        | `/runs`        | every run ([Lists of runs](#lists-of-runs))                 |
| run         | `/runs/ID`     | run ID's transcripts ([The run page](#the-run-page))         |
| automations | `/automations` | the automations ([The automations page](#the-automations-page)) |
| server      | `/server`      | the facts about Linewright itself ([The server page](#the-server-page)) |

### Serving

`serve` serves the web UI on `ui-address`, a host and port, and serves none
when `ui-address` is empty.

- **Access.** Whoever reaches the port; who reaches it is the operator's to
  restrict, such as with an authenticating proxy in front.
- **Requests answered.** Only those whose `Host` is local
  ([Commands](monitoring.md#commands)), an IP address or one of
  `ui-hosts`, each compared ignoring case, port and a trailing dot. The
  others get 421 Misdirected Request.

### Navigation

| Part       | Holds                                                                                   |
| ---------- | --------------------------------------------------------------------------------------- |
| repository | the avatar of the GitHub account that owns it, beside its name, linking to it on GitHub |
| links      | the board, the runs, the automations and the server pages                               |

### Holds and problems

| Hold or problem                          | Links to         |
| ---------------------------------------- | ---------------- |
| a usage limit in force                   | the server page  |
| a drain under way                        | the server page  |
| a webhook delivery refused since the last one accepted | the server page |
| a poll that failed since the last that succeeded | the server page |
| an automation file that does not parse   | the automations page |
| a file of the repository's Linewright cannot use | the server page |
| an automation whose last run failed      | that run         |

### The board

The board shows managed items as cards in six columns, in this order:

| Column         | Holds a managed item that                                                                            |
| -------------- | ---------------------------------------------------------------------------------------------------- |
| Triage         | holds `needs-triage`                                                                                 |
| Needs a person | holds `needs-info` or `ready-for-human`                                                              |
| Ready          | holds `ready-for-agent` and is not claimed, or is a spec change not yet designed                     |
| Working        | is claimed for design or implementation, or has a run in flight that no other column shows           |
| In review      | has an open pull request that closes it, or is a pull request of Linewright's that closes no issue on the board |
| Done           | is worked, and is closed or has had a pull request that closes it merge into the default branch since it was last opened |

- **Two columns.** An item that fits two columns is in the one further
  right.
- **One card per piece of work.** The card is the issue; a pull request, and
  a run on it, show on the card of each issue it closes.
- **A spec change.** Until it is designed, its card is its pull request;
  designed, it is its tracking issue's lane where one was cut, and otherwise
  its pull request, in Done, saying there was nothing to build.
- **Lanes.** A lane per piece of work: where the work was cut, headed by its
  tracking issue's number and title and its tickets closed of total, with
  its tickets as its cards; otherwise headed by the item's number and title,
  with the item as its one card.
- **Finished lanes.** A lane whose cards are all in Done is finished. It shows
  as its head alone, with when it finished: when its last card entered Done, or,
  for a spec change designed with nothing to build, when it was designed. It
  opens on click. The finished lanes sit together under one fold that says
  how many there are and opens on click. A finished lane leaves the board
  once it finished before the Done window (`done-window`) opened.
- **Order of lanes.** The unfinished lanes come first, latest changed first,
  then the finished lanes, latest finished first. Lanes changed at the same
  time are in number order.
- **A lane's last change.** The latest thing that happened, on GitHub or in
  Linewright, to any of its cards or to a pull request that closes one. A
  run in flight is happening now.
- **Order in a column.** Ascending by number, within each lane.
- **No cap.** A column is never cut.
- **Needing a person.** The browser tab's title carries the count of cards in
  Needs a person.

### Cards

A card is the item's number and title and, under them, at most one line that
says what its column does not: what sets the item apart from the rest of its
column, what a person acts on, or since when it has sat. A card with nothing
to add is the title alone.

- **Pull requests.** A card that is a pull request carries a pull request
  icon beside its number.
- **The first that holds.** Where more than one row of a column holds, the
  line is the first.
- **Card status.** A card whose line has a card status shows it. Card
  statuses, most pressing first: failed, waiting for a person, in flight,
  paused or queued.
- **Done.** A card in Done is quieter than the rest.
- **When.** In Triage, Needs a person, Ready and Done, the line ends with
  when the item entered its column, or when it was created where that is not
  known.

| Column         | The line                                                                                                                                                                                                                                                               | When                                                                                                                                                    | Card status |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| Triage         | nothing but the time                                                                                                                                                                                                                                                   |                                                                                                                                                         |  |
| Needs a person | "stalled"                                                                                                                                                                                                                                                              | it holds `linewright:stalled`                                                                                                                           | failed |
|                | "asked alice", "asked alice and bob", "asked alice, bob and carol": the issue's assignees, in GitHub's order; "people asked" with none                                                                                                                                 | it holds `needs-info`                                                                                                                                   | waiting for a person |
|                | "handed back"                                                                                                                                                                                                                                                          | it holds `ready-for-human`, and Linewright's last act on it was letting go of it: a run handed it back, or its pull request closed unmerged              | waiting for a person |
|                | "decision needed"                                                                                                                                                                                                                                                      | it holds `ready-for-human` otherwise                                                                                                                    | waiting for a person |
| Ready          | "blocked by #12"                                                                                                                                                                                                                                                       | an open issue blocks it                                                                                                                                 |  |
|                | "assigned to alice"                                                                                                                                                                                                                                                    | someone other than Linewright is assigned to it                                                                                                         |  |
|                | "unassigned"                                                                                                                                                                                                                                                           | Linewright was unassigned from it                                                                                                                       |  |
|                | "paused for the usage limit", "paused for the drain"                                                                                                                                                                                                                   | it would start, but for the hold                                                                                                                        | paused or queued |
|                | "queued"                                                                                                                                                                                                                                                               | it would start, but the runs in flight are at `max-runs`                                                                                                | paused or queued |
|                | nothing but the time                                                                                                                                                                                                                                                   | otherwise: the next poll acts on it                                                                                                                     |  |
| Working        | its stage and how long it has run, "implementation · 12m"                                                                                                                                                                                                              | a run is in flight                                                                                                                                      | in flight |
|                | "paused for the usage limit", "paused for the drain", "queued"                                                                                                                                                                                                         | no run is in flight, as in Ready                                                                                                                        | paused or queued |
| In review      | its pull request, where the card is not the pull request itself, then what stops it merging, the first of the table below that holds | always; a pull request opened by someone else is "#42 by alice" | that of what stops it merging |
| Done           | "closed in #42", the first pull request that closes it to merge since it was last opened                                                                                                                                                                              | an issue a merged pull request closes                                                                                                                   |  |
|                | "closed as completed", "closed as not planned" or "closed as a duplicate", GitHub's reason; "closed" where the reason is not known                                                                                                                                     | an issue closed otherwise                                                                                                                               |  |
|                | "merged"                                                                                                                                                                                                                                                               | a pull request of Linewright's merged                                                                                                                   |  |
|                | "closed without merging"                                                                                                                                                                                                                                               | a pull request of Linewright's closed unmerged                                                                                                          |  |
|                | "merged · nothing to build"                                                                                                                                                                                                                                            | a spec change designed with nothing to build                                                                                                            |  |

What stops a pull request merging, in the order the line picks from:

| What stops it merging                                     | When                                       | Card status |
| --------------------------------------------------------- | ------------------------------------------ | --- |
| "stalled"                                                 | it holds `linewright:stalled`              | failed |
| its stage and how long it has run, as in Working          | a run is in flight on it                   | in flight |
| "checks failed"                                           | a check failed                             | failed |
| "checks running"                                          | a check has not finished                   | in flight |
| "conflicts"                                               | it cannot merge cleanly                    | failed |
| "behind main", naming the default branch                  | it is behind the default branch            |  |
| "draft"                                                   | it is a draft                              |  |
| "changes requested"                                       | GitHub's review decision is that changes are requested | waiting for a person |
| "review required"                                         | GitHub requires a review before it merges, such as a code owner's, that it has not had | waiting for a person |
| "approved"                                                | GitHub's review decision is approved    |  |
| nothing                                                   | otherwise                                  |  |

### The item page

The item page shows #N's heading, then its facts ([Facts](#facts)) in the
order below, then its runs ([Lists of runs](#lists-of-runs)).

- **Heading.** #N's number and title as on its card, and under them when
  `serve` last read #N, such as "as of 2 minutes ago".
- **Its card.** For an item shown on another's card, such as a pull request
  on the card of the issue it closes, the card it would have on its own.
- **Related items.** An item a fact lists is shown as its card is, with its
  card status's icon in place of its line, and a tracking issue as its lane
  head is.

| Fact           | Holds                                                                                                                                  | Shown                          |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ |
| State          | #N's column, then its card's line, with its card status ([Cards](#cards)); for a tracking issue, "Tracking", and once its lane is finished when it finished, on the board or off it, such as "Tracking · finished 2 days ago"; for an item the board does not show, "Left alone", then what keeps it out, such as "closed", "not marked for Linewright" or "waits on a person, and Linewright has not worked it" | always                         |
| Waiting for    | each thing Linewright waits for before it acts on #N next, one per line, naming what ends it, such as "the usage limit, until 14:30", in plain text | while it waits for any         |
| Tracking issue | a ticket's tracking issue, or the tracking issue cut from a spec change                                                                | while there is one             |
| Tickets        | a tracking issue's tickets, with how many are closed of the total, such as "3 of 5 closed"                                             | while it has any               |
| Spec change    | the pull request of the spec change a tracking issue tracks                                                                            | while there is one             |
| Pull requests  | the open or merged pull requests that close #N                                                                                         | while there are any            |
| Closes         | the issues a pull request closes                                                                                                       | while there are any            |

### Lists of runs

A list of runs is a table with a row per run, newest first.

| Column   | Holds                                                                                                                  |
| -------- | ---------------------------------------------------------------------------------------------------------------------- |
| Outcome  | the run's outcome                                                                                                      |
| Run      | the run ID, linking to the run page                                                                                    |
| Item     | the item; for an automation's run, "automation" and its name, linking to the automations page |
| Stage    | the run's stage                                                                                                        |
| Started  | when the run started, with an automation's run by hand marked there                                                    |
| Duration | the run's duration                                                                                                     |

- **On the runs page.** Filters for the item, the stage and the time since
  which runs started narrow the list.
- **On the item page.** The list leaves out the Item column.
- **No runs.** "No runs" stands in place of the table.
- **A cut list.** The item page shows #N's latest ten runs. A last row,
  across the table, says how many earlier runs were left out and links to the
  runs page narrowed to #N.

### The run page

The run page shows the run's transcripts ([Transcripts](#transcripts)), each
under its session's model and effort and where its settings and prompt files
came from.

- **Heading.** The run ID, then ", by hand" for an automation's run by hand
  and "(it started no session)" for a run that ended without one, and under
  them the run's item as in the Item column of [Lists of runs](#lists-of-runs).
- **Folding.** An entry that runs opens on click to the rest of it, a tool
  call's full input and result or the setup command's output, and one that
  failed starts open. A sub-agent's work starts folded to the line of the
  call that started it, with the count of its entries.
- **Verbose.** A switch, set once for the page, shows the transcripts in full,
  with everything open.
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

### The server page

The server page shows the facts about Linewright itself ([Facts](#facts)).

| Fact                                                            | Shown                  |
| --------------------------------------------------------------- | ---------------------- |
| since when `serve` has run                                      | always                 |
| the last poll and the next                                      | always                 |
| the polls failed since the last that succeeded, with the last error | while there are any |
| the usage limit and when it resets                              | while one is in force  |
| a file of the repository's Linewright cannot use, and its error | while there is one     |
| the drain, since when, and its deadline if set                  | while one is under way |
| the runs in flight out of `max-runs`                            | always                 |
| the time of the last webhook delivery accepted                  | with webhooks on       |
| the webhook deliveries refused since `serve` started            | with webhooks on       |

## Out of scope

- A login of Linewright's own for the web UI.
- A layout of the board for narrow screens.
- A status of a lane's own: its cards carry theirs.
- Acting from the UI on `serve` or on the work.
