# Design system

## Liveness

- A page shows `serve`'s state as it changes, without the reader reloading
  it.
- A page that loses `serve` keeps what it last showed, says since when, and
  reconnects on its own.
- What the reader opened or closed stays so through updates, until they
  open the page again.

## Navigation

- Every page has the same navigation.
- A hold on Linewright's work, such as a usage limit, or a problem no item
  shows, is shown on every page while it lasts, linking to the page that
  tells more.
- Every item or pull request named on a page links to its page and to
  GitHub.

## Time

- A past time is shown relative, such as "5 min ago", keeping up as time
  passes, with the absolute time in the reader's time zone shown on hover.
- A future time is shown absolute, in the reader's time zone.
- A duration reads the same on every page.

## Status

- An outcome or a state has an icon and a colour, shared by those that mean
  the same, and is never told by colour alone.

## Board

- As the reader scrolls, the column heads stay at the top, with the head of
  the lane they are in under them.
- A card's title is on at most two lines, and whole on hover.

## Colour scheme

- Light or dark follows the reader's preference.

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
- **Lanes.** A lane per piece of work: where the work was cut, headed by its
  tracking issue's number and title and its tickets closed of total, with
  its tickets as its cards; otherwise headed by the item's number and title,
  with the item as its one card.
- **Finished lanes.** A lane whose cards are all in Done is finished. It shows
  as its head alone, with when its last card closed, and opens on click. The
  finished lanes sit together under one fold that says how many there are
  and opens on click.
- **Order of lanes.** The lanes with a card in Needs a person come first,
  then the other unfinished lanes, then the finished lanes; unfinished lanes
  in number order, and finished lanes latest finished first.
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
| the line         | its run in flight, as the stage and how long it has run; or else each thing it waits for, as `explain` words it; or, in Done, how it ended: merged by which pull request, closed without merging, or closed |
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

- A layout of the board for narrow screens.
- Acting from the UI on `serve` or on the work.
