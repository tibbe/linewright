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
- Every item or pull request named on a page links to its page and to
  GitHub.

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

## Layout

- Content wider than the page scrolls sideways on its own, not the page with
  it.
- What a page shows on hover it also shows on keyboard focus, over the
  page, until the pointer leaves it or focus moves. Text cut to fit is shown
  whole this way.

## Colour scheme

- Light or dark follows the reader's preference.

## The web UI

`serve` serves a web UI for the maintainers: the facts the monitoring
commands show ([Monitoring](monitoring.md)), chosen and laid out for
someone scanning a page, in the UI's own words rather than a command's.

| Page        | Path           | Shows                                                       |
| ----------- | -------------- | ----------------------------------------------------------- |
| board       | `/`            | the managed items as cards ([The board](#the-board))        |
| item        | `/items/N`     | where #N stands, what it waits for and its runs ([Lists of runs](#lists-of-runs)) |
| runs        | `/runs`        | every run ([Lists of runs](#lists-of-runs))                 |
| run         | `/runs/ID`     | what `logs` shows of run ID ([The run page](#the-run-page))  |
| automations | `/automations` | the automations ([The automations page](#the-automations-page)) |
| server      | `/server`      | the facts about Linewright itself ([The server page](#the-server-page)) |

### Serving

`serve` serves the web UI on `ui-address`, a host and port, and serves none
when `ui-address` is empty.

- **Requests answered.** Only those whose `Host` is local
  ([The local API](architecture.md#the-local-api)), an IP address or one of
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
- **Lanes.** A lane per piece of work: where the work was cut, headed by its
  tracking issue's number and title and its tickets closed of total, with
  its tickets as its cards; otherwise headed by the item's number and title,
  with the item as its one card.
- **A lane's card status.** A lane's head ends with the line of its most
  pressing card: the first card, by column and then by number, with the most
  pressing card status any of its cards has. The line follows the card's
  number where the card is not the lane's own item.
- **Finished lanes.** A lane whose cards are all in Done is finished. It shows
  as its head alone, with when its last card closed, and opens on click. The
  finished lanes sit together under one fold that says how many there are
  and opens on click.
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
- **When.** In Done, the line ends with when the item closed. In Triage,
  Needs a person and Ready, it ends with when the item entered its column,
  or when it was created where that is not known.

| Column         | The line                                                                                                                                                                                                                                                               | When                                                                                                                                                    | Card status |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| Triage         | nothing but the time                                                                                                                                                                                                                                                   |                                                                                                                                                         |  |
| Needs a person | "stalled"                                                                                                                                                                                                                                                              | it holds `linewright:stalled`                                                                                                                           | failed |
|                | "asked alice", "asked alice and bob", "asked alice, bob and carol": the issue's assignees, in GitHub's order; "people asked" with none                                                                                                                                 | it holds `needs-info`                                                                                                                                   | waiting for a person |
|                | "handed back"                                                                                                                                                                                                                                                          | it holds `ready-for-human`, and Linewright's last act on it was letting go of it: a run handed it back, or its pull request closed unmerged              | failed |
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
| Done           | "closed in #42"                                                                                                                                                                                                                                                        | an issue closed by a merged pull request                                                                                                                |  |
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
| "changes requested", "review required" or "approved"      | the review decision, as GitHub words it    | waiting for a person, but none for "approved" |

### Lists of runs

A list of runs is a table with a row per run, newest first.

| Column   | Holds                                                                                                                  |
| -------- | ---------------------------------------------------------------------------------------------------------------------- |
| Outcome  | the run's outcome                                                                                                      |
| Run      | the run ID, linking to the run page                                                                                    |
| Item     | the item's number, and its title where Linewright knows it; for an automation's run, "automation" and its name, linking to the automations page |
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

The run page shows a run's transcript as `logs` shows it, under each
session's model and effort and where its settings and brief files came from.

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

### The server page

The server page shows the facts about Linewright itself.

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
- Acting from the UI on `serve` or on the work.
