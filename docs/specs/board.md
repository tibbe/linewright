# Board

Work on a repository is spread over many issues and pull requests, and each
waits on something different: an agent, a check, a person. A maintainer needs
to see where each piece of work stands, and which of it waits for them,
without opening each item on GitHub. The board shows all the work Linewright
manages on one screen, as cards in the columns of their states, and the Item
screen and `lw explain` say where one item stands and why.

For example, the operator wonders why #42 has not moved in an hour.
`lw explain 42` shows a rework run on it that has gone for 50 minutes,
and `lw logs 42 --follow` shows the agent waiting on a test that hangs.

## User Stories

1. As a maintainer, I want to see all the work Linewright manages on one
   screen, with what needs a person apart from the rest, so that I know where
   to act.
2. As a maintainer, I want to narrow the board to the work I am party to, so
   that I find mine quickly.
3. As a maintainer, I want to see where my spec change or issue stands and
   what it waits for, as soon as it changes, so that I do not wait on GitHub
   for news.
4. As an operator, I want to ask why an issue or pull request is not moving,
   including why Linewright is not working on it at all, so that I can unblock
   it.

## Naming items

- Every item or pull request named on a screen is named by its number, and
  its title where Linewright knows it, except in a card's line, which names
  it by its number alone, and links to its [Item](#item) and to GitHub.

## States

An item's state ([State](repository.md#state)) is shown in these words.

| State             | Shown as               |
| ----------------- | ---------------------- |
| `needs-triage`    | Waiting for triage     |
| being triaged     | Being triaged          |
| `needs-info`      | Waiting for an answer  |
| `ready-for-human` | Waiting for a decision |
| `ready-for-agent` | Waiting for Linewright |
| being designed    | Being designed         |
| being implemented | Being implemented      |
| in review         | In review              |
| tracking          | Tracking               |
| done              | Done                   |
| open              | Open                   |
| merged            | Merged                 |
| closed            | Closed                 |

## `lw explain`

`explain N` brings together what Linewright knows about #N, and points to
`runs` and `logs` for the detail.

- **What it shows.** For issue or pull request #N:
  - its parents and its children in the pipeline, such as the issues a pull
    request closes, or the pull requests that close a ticket;
  - its state ([States](#states)), whether it is stalled,
    and when `serve` last read it;
  - each thing Linewright waits for before it acts on #N next, naming what
    ends the wait, such as "the usage limit, until 14:30 UTC", "the drain,
    until `serve` restarts" or "the environment, until it is built"
    ([Holds](runs.md#holds));
  - its run in flight;
  - its latest three past runs, as `runs N` lists them.
- **An item Linewright leaves.** For an item that is not managed,
  `serve` reads #N from GitHub and `explain` gives its state as for a managed
  item, where it has one, and says why Linewright leaves it, naming the fact
  or setting that keeps it out.
- **State in JSON.** An item's state is its name under
  [State](repository.md#state), in kebab-case, such as `being-implemented`,
  and null for an item with no state.

## Screens

| Screen | Reached from                                                    | Purpose                                                                             |
| ------ | --------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Board  | the navigation ([Shell](shell.md)), and opening the UI | see where every managed item stands, and which need a person                        |
| Item   | an item named on any screen ([Naming items](#naming-items))     | see where one item stands, what it waits for, the items it relates to and its runs |

### Board

- **Address.** `/`.
- **Layout.** The board is
  six columns side by side. Lanes cross them, one under another, a lane per
  piece of work. Each card is in its lane and in the column of its item's state
  ([State](repository.md#state)). As the reader scrolls, the column heads stay
  at the top, with the head of the lane they are in under them.
- **Filters.** Person ([Filtering by person](#filtering-by-person)).
- **Columns.** The columns below, in this order. The columns share the
  board's width, each at least 200px, and all six fit beside the navigation
  rail from 1440px wide.
- **The column heads** are a row in `bg-base-100`, `h-8` tall, with each
  column's name in `font-semibold`.
- **No cap.** A column is never cut.
- **Needing a person.** The screen's title carries the count of cards in Needs a
  person on the whole board, however it is filtered.

| Column         | An issue's states                                | A pull request's states |
| -------------- | ------------------------------------------------ | ----------------------- |
| Triage         | `needs-triage`                                   |                         |
| Needs a person | `needs-info`, `ready-for-human`                  |                         |
| Ready          | `ready-for-agent`                                |                         |
| Working        | being triaged, being designed, being implemented |                         |
| In review      | in review                                        | open                    |
| Done           | done                                             | merged, closed          |

- **What has a card.** Each managed item
  ([Managed items](repository.md#managed-items)). A tracking issue has no
  card.
- **One card per piece of work.** The card is the issue. A pull request, and
  a run on it, show on the card of each issue that it closes and that has a
  card. A pull request that closes no issue with a card has its own card.
- **Order in a column.** Ascending by number, within each lane.

#### Lanes

- **A lane.** Where the work was cut into tickets, the lane's head shows the
  tracking issue's number and title, and how many of its tickets are closed
  of the total. Its cards are its managed tickets. Otherwise, the head shows
  the item's number and title, and the item is its one card.
- **Its look.** A lane stands apart from the screen around it, and each of
  its cards stands apart from the lane. A lane is drawn as a card is
  ([Cards](ui_patterns.md#cards)), with `px-2` at its sides and `pb-2` at its
  foot. In a lane, the columns are `gap-2` apart, and so are the cards in a
  column.
- **A lane's head** is its top row, in `bg-base-200`, at least `h-10` tall,
  with `pt-2` above its text. Its text is in `font-semibold`, on one line, cut
  to fit. It is not a `card-title`, so it has no icon before its text.
- **On a lane or a card.** A lane or a card holds no daisyUI component that
  draws in `base-100`, `base-200` or `base-300`, such as a button, an input
  or an alert.
- **Finished lanes.** A lane is finished once its cards are all in Done and,
  where it has a tracking issue, that issue is done. It shows as its head
  alone, with when it finished: when the last of its cards entered Done or
  its tracking issue became done. It opens on click. The finished
  lanes sit together under one fold that says how many there are, opens
  on click and shows only while there is one. The finished lanes are under a
  daisyUI `divider`. Their fold, and each finished lane, is a daisyUI
  `collapse` with `collapse-arrow`.
  A finished lane leaves the board when it finished before the start of the
  Done window (`done-window`).
- **Order of lanes.** The unfinished lanes come first, latest changed first,
  then the finished lanes, latest finished first. Lanes changed at the same
  time are in number order.
- **A lane's last change.** The latest thing that happened, on GitHub or in
  Linewright, to any of its cards or to a pull request that closes one. A
  run in flight counts as a change at the current time.

#### Cards

A card is the item's number and title and, under them, at most one line that
says what its column does not: what sets the item apart from the rest of its
column, what a person acts on, or since when it has sat. A card with nothing
to add is the title alone.

- **Its look.** A card is a daisyUI `card`, `card-sm`, drawn as a card is
  ([Cards](ui_patterns.md#cards)), but in `bg-base-100`.
- **The title.** A card's title is on at most two lines.
- **Issue or pull request.** Beside its number, a card carries an icon that
  says whether it is an issue or a pull request: Octicons' `issue-opened` or
  `git-pull-request`.
- **The first that holds.** Where more than one row of a column holds, the
  line is the first.
- **Card status.** A card whose line has a card status shows it. Card
  statuses, most pressing first: failed, waiting for a person, in flight,
  paused or queued. Each shows as a status
  ([Status](ui_patterns.md#status)): failed as failed, waiting for a person
  as waiting, in flight as in flight, and paused or queued as paused. A card
  that shows a card status has an edge `3px` wide at its start in its colour,
  and its icon, in `base-content`, before its line.
- **Done.** A card in Done is quieter than the rest, at `opacity-60`.
- **How it ended.** An item in Done shows as done where it merged or a merged
  pull request closed it, and as cancelled where it closed otherwise.
- **When.** In Triage, Needs a person, Ready and Done, the line ends with
  when the item entered its column, or when it was created where that is not
  known.

| Column         | The line                                                                                                                                                                                                                                                               | When                                                                                                                                                    | Card status |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | --- |
| Triage         | as in Ready                                                                                                                                                                                                                                                            | as in Ready                                                                                                                                             | as in Ready |
| Needs a person | "stalled"                                                                                                                                                                                                                                                              | it holds `linewright:stalled`                                                                                                                           | failed |
|                | "asked alice", "asked alice and bob", "asked alice, bob and carol": the issue's assignees, in GitHub's order; "people asked" with none                                                                                                                                 | it holds `needs-info`                                                                                                                                   | waiting for a person |
|                | "handed back"                                                                                                                                                                                                                                                          | Linewright's last act on it was letting go of it: a run handed it back, or its pull request closed unmerged                                             | waiting for a person |
|                | "decision needed"                                                                                                                                                                                                                                                      | otherwise                                                                                                                                               | waiting for a person |
| Ready          | "blocked by #12"                                                                                                                                                                                                                                                       | an open issue blocks it                                                                                                                                 |  |
|                | "assigned to alice"                                                                                                                                                                                                                                                    | someone other than Linewright is assigned to it                                                                                                         |  |
|                | "unassigned"                                                                                                                                                                                                                                                           | Linewright was unassigned from it                                                                                                                       |  |
|                | "not marked for Linewright"                                                                                                                                                                                                                                            | it is not marked for Linewright                                                                                                                         |  |
|                | "paused until" what ends the hold ([Holds](runs.md#holds)), naming a file: "paused until the environment is built", "paused until the default branch fixes `.linewright/config.toml`"                                                                                  | it would start, but for a hold                                                                                                                          | paused or queued |
|                | "queued"                                                                                                                                                                                                                                                               | it would start, but the runs in flight are at `max-runs`                                                                                                | paused or queued |
|                | nothing but the time                                                                                                                                                                                                                                                   | otherwise: the next poll acts on it                                                                                                                     |  |
| Working        | its stage and how long it has run, "implementation · 12m"                                                                                                                                                                                                              | a run is in flight                                                                                                                                      | in flight |
|                | "blocked by #12"                                                                                                                                                                                                                                                       | an open issue blocks it                                                                                                                                 |  |
|                | "paused until" what ends the hold, or "queued"                                                                                                                                                                                                                         | no run is in flight, as in Ready                                                                                                                        | paused or queued |
| In review      | its pull request, where the card is not the pull request itself, then what stops it merging, the first of the table below that holds | always; a pull request opened by someone else is "#42 by alice" | that of what stops it merging |
| Done           | "closed in #42", the pull request whose merge made it done                                                                                                                                                                                                             | an issue a merged pull request made done                                                                                                                |  |
|                | "closed as completed", "closed as not planned" or "closed as a duplicate", GitHub's reason; "closed" where the reason is not known                                                                                                                                     | an issue closed otherwise                                                                                                                               |  |
|                | "merged"                                                                                                                                                                                                                                                               | a pull request merged                                                                                                                                   |  |
|                | "closed without merging"                                                                                                                                                                                                                                               | a pull request closed unmerged                                                                                                                          |  |

What stops a pull request merging, in the order the line picks from:

| What stops it merging                                     | When                                       | Card status |
| --------------------------------------------------------- | ------------------------------------------ | --- |
| "stalled"                                                 | it holds `linewright:stalled`              | failed |
| "handed back"                                             | Linewright opened it, is not assigned to it, and its last act on it was letting go of it | waiting for a person |
| "unassigned"                                              | Linewright opened it and is not assigned to it otherwise | |
| its stage and how long it has run, as in Working          | a run is in flight on it                   | in flight |
| "paused until" what ends the hold, or "queued", as in Ready | Linewright would start a run on it, as in Ready | paused or queued |
| "checks failed"                                           | a check failed                             | failed |
| "checks running"                                          | a check has not finished                   | in flight |
| "conflicts"                                               | it cannot merge cleanly                    | failed |
| "behind main", naming the default branch                  | it is behind the default branch            |  |
| "draft"                                                   | it is a draft                              |  |
| "changes requested"                                       | GitHub's review decision is that changes are requested | waiting for a person |
| "review required"                                         | GitHub requires a review before it merges, such as a code owner's, that it has not had | waiting for a person |
| "approved"                                                | GitHub's review decision is approved    |  |
| nothing                                                   | otherwise                                  |  |

#### Filtering by person

The board has one filter ([Filters](ui_patterns.md#filters)), Person: it
narrows the board to the cards that match a person. A card matches whoever
is party to its item, to a pull request on it or, for a ticket, to its
tracking issue: a person who opened it, is assigned to it or reviews it.

- **Opening.** Linewright opens some items for another item, such as a spec
  gap for the item whose work found it, or a spec change's issue for the spec
  change. Such an item counts as opened by the person who opened that other
  item, or the spec change's author. Linewright follows this back until it
  reaches an account other than its own. An item that Linewright opened for
  no other item, such as an issue that an automation files, stays
  Linewright's.
- **Linewright.** Linewright is party to nothing.
- **Reviewing.** A person reviews a pull request while a review is
  requested of them by name, and once they have reviewed it.
- **Lanes.** A lane shows while any of its cards matches.
- **Lane heads.** A lane's head, with its tickets closed of total, is the
  whole lane's.
- **The people.** Person is a list of logins: each person some card on the
  whole board matches, however it is filtered, and whoever is chosen. It
  has nothing to type in. It is a `select`.

### Item

- **Address.** `/items/N`.
- **Layout.** #N's heading at the top, then its facts
  ([Facts](components.md#facts)) in the order below, then its runs.
- **Heading.** #N's number and title as on its card, and under them when
  `serve` last read #N, such as "as of 2 minutes ago".
- **Runs.** #N's latest ten runs, as a list of runs ([Runs](runs.md#runs-1))
  without the Item column. A last row, across the table, says how many earlier
  runs were left out and links to Runs narrowed to #N.
- **Its card.** On this screen, an item without a card of its own, such as a
  pull request on the card of the issue that it closes, has the card it
  would have on its own. A tracking issue's card is its lane head.
- **Related items.** An item that a fact lists shows as its card does, with
  its card status's icon in place of its line. A tracking issue shows as its
  lane head does.

| Fact           | Holds                                                                                                                                  | Shown                          |
| -------------- | -------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------ |
| State          | #N's state ([States](#states)), then its card's line where it says more, with its card status ([Cards](#cards)), such as "Being implemented · implementation · 12m"; for a tracking issue, its state, then, once its lane is finished, when it finished, such as "Done · finished 2 days ago"; for an item that is not managed, its state where it has one, then "Left alone" and what keeps it out ([Managed items](repository.md#managed-items)), such as "not marked for Linewright" | always                         |
| Waiting for    | each thing Linewright waits for before it acts on #N next, one per line, naming what ends it ([Holds](runs.md#holds)), such as "the usage limit, until 14:30", in plain text | while it waits for any         |
| Tracking issue | a ticket's tracking issue                                                                                                              | while there is one             |
| Tickets        | a tracking issue's tickets, with how many are closed of the total, such as "3 of 5 closed"                                             | while it has any               |
| Spec change    | the spec change a spec change's issue was opened for ([Spec changes](sources_of_work.md#spec-changes))                                  | while there is one             |
| Pull requests  | the open or merged pull requests that close #N                                                                                         | while there are any            |
| Closes         | the issues a pull request closes                                                                                                       | while there are any            |

## Out of scope

- A layout of the board for narrow screens.
- A status of a lane's own: its cards carry theirs.
