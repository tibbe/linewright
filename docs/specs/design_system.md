# Design system

## Liveness

- A screen shows `serve`'s state as it changes, without the reader opening
  it again.
- What the reader opened or closed stays so through updates, until they
  open the screen again.

## Words

- **What is so.** A screen states what is so, not what to do about it; how to
  work with Linewright is the user guide's.
- **Voice.** A sentence on a screen is in the active voice: it names who or
  what acts, such as "This stops its running trials and scorers."
- **Buttons.** A button's label is a verb, or "New" for a button that makes
  something, but for the buttons of a table's pages ([Tables](#tables)). The
  label also names what the button acts on where the screen does not show it.
  For example, "Remove" on a task's row and "Cancel" on Benchmark run need no
  more, but "Add task" on Run names the task.

## Titles

- **A screen's title** is the name in its heading, then "Linewright", such as
  "Benchmark run 14 · Linewright". For a screen of the navigation, the name is
  the screen's name in the navigation.

## Headings

- **A heading.** A screen of one thing, such as a run, has a heading in a card
  of its own. The card is under the screen's breadcrumbs, or at its top where it
  has none. The heading holds the thing's name, then the badge of its state
  where the screen gives its state there. Under the name is the line that the
  screen gives, if any.

## Screen states

- **Empty.** A list with nothing in it shows "No" and what it lists in its
  place, such as "No runs" or "No benchmarks", unless its screen shows it only
  while it holds something.
- **Lost.** A screen that loses `serve` keeps what it last showed, shows
  "Lost `serve` 5 minutes ago; reconnecting.", with the time as a past time
  ([Time](#time)), and reconnects on its own.
- **Nothing there.** A link can name an item, run, benchmark or benchmark run
  that does not exist. The screen it opens shows "No such" and the name, as the
  screens give it, such as "No such run 43/rework-9" or "No such item #812".
- **Absent.** A figure that is absent, such as a mean with nothing to
  count, shows as "–".
- **Loading.** A screen shows once it has all that it holds; it has no
  loading state.

## Filters

- **Where.** A screen's filters sit together above what they narrow, each
  labelled with what it filters on.
- **Applying.** A filter applies as soon as it is chosen from a list, and
  one typed in when the reader presses Enter.
- **Unset.** An unset filter narrows nothing: a list reads "All" and what it
  lists, such as "All stages", and a field to type in is empty.
- **Narrowing.** What the filters leave out is gone from the screen and from
  its counts, but for a count the screen says holds however it is filtered.
- **Links.** A screen's link carries its filters: a reader who opens the
  screen again, or follows its link, gets the same filters. A link to the
  screen from elsewhere, such as from the navigation, carries none unless it
  says.
- **Kept.** An update never changes a filter.
- **Opened and closed.** Changing a filter leaves what the reader opened or
  closed as it was.

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

- **Statuses.** A status is one of done, failed, waiting, in flight, paused
  and cancelled. Each area says which status each of its states shows as.
- **Shown.** A status shows as its icon and its colour, never by colour alone.
  Where a screen also names a status's state, a badge in the status's colour
  holds the icon and the state's name. A status among a screen's facts is the
  exception ([Facts](#facts)). For example, a running benchmark run shows as
  "running", with the icon of in flight, in its colour.

## Tables

- **The card.** A table is in a card. The top of the card holds the table's name
  at the left and the table's buttons at the right. The table's filters are in a
  row of their own under the top. Unless its screen names it, a table's name
  says what its rows are, such as "Runs".
- **A table in a tab.** A table that fills a tab has the tab's name.
- **No name shown.** The card of a table that fills a tab, or of the table
  that a screen of the navigation lists, shows no name: the tab or the
  breadcrumbs name it.
- **A row's buttons.** A row has its buttons at its end, but for the button
  that opens it in place. No part of a row but a button or a link acts on a
  click.
  - **A row's screen.** A row that stands for a thing with a screen of its
    own has "View" at its end, a link to that screen. A screen reader reads
    the row's name after it, such as "View run 148/implement-1".
  - **Opening in place.** A row that opens in place, such as to rows under it,
    starts with a button that shows a chevron. The chevron points right while
    the row is closed and down while it is open. A screen reader reads the
    button as what it opens, such as "Show the tasks of opus-high". A row with
    nothing to open to has no button there, and its first cell keeps its place.
- **The first column.** A first column that names each row has a head. Its
  cells are the rows' headers, in bold.
- **Alignment.** Amounts of money and durations align right. Every other
  column aligns left.
- A title in a table is on one line.
- **Pages.** A table with more than 25 rows shows them 25 at a time. Only the
  table's own rows count, not the rows that they open to.
  - **Under the table.** At the left is which rows show, of how many, such as
    "Showing 26–50 of 248 runs". At the right are "Previous", the numbers of the
    pages and "Next". The numbers are those of the first page, the last and the
    two on each side of the reader's. "…" is in place of the numbers between
    them. "Previous" is disabled on the first page, and "Next" on the last.
  - **Links.** A screen's link carries its pages, as it carries its filters
    ([Filters](#filters)). Changing a filter goes back to the first page.
  - **Updates.** When the table gets rows or loses them, the reader stays on the
    page that they are on. A page that no longer exists shows as the last page.

## Facts

- A label is words alone, with no icon or colour. A status among a screen's
  facts is in the value: its icon and its state's name in its colour, with
  nothing behind them.

## Dialogs

- **Where.** A dialog opens over the whole page, the shell too
  ([Shell](shell.md)). It has no link and no breadcrumbs of its own.
- **Closing.** A click outside a dialog does nothing. Escape closes it, as
  its back-out button does.

## Confirming

- **What asks first.** An action that stops work or removes what Linewright
  cannot bring back asks first, in a confirmation, a dialog
  ([Dialogs](#dialogs)): a question that names
  what it acts on and says what follows.
- **The answer.** The reader confirms, and the action goes ahead, or turns
  it down, and nothing changes.
- **Its buttons.** The button that goes ahead comes last. It has the label of
  the button that asked, and is in the Error colour. The other comes first and
  says "Cancel", or "Keep running" where the action cancels.

## Making

- **New.** A table that the reader can add to has "New" with its screen's
  buttons ([Shell](shell.md)), or, for a table in a tab, in its card
  ([Tables](#tables)).
- **The dialog.** "New" opens a dialog ([Dialogs](#dialogs)). The dialog
  shows each field with its label above it, then "Cancel" and the button that
  makes the thing.
- **Making.** When the reader clicks the button that makes the thing and
  Linewright makes it, the dialog closes.
- **A refusal.** When Linewright refuses, the dialog stays open and shows the
  reason above its buttons. The fields keep what the reader typed or chose.

## Layout

- The UI is laid out for someone scanning a screen.
- Content wider than the screen scrolls sideways on its own, not the screen
  with it.
- What a screen shows on hover it also shows on keyboard focus, over the
  screen, until the pointer leaves it or focus moves. Text cut to fit is shown
  whole this way.
- Only a name's title is cut to fit; its number and its link to GitHub
  always show.

## Type

- Secondary text, text that qualifies the text it sits beside, such as a
  card's line under its title, a fact's label or a transcript entry's
  time or duration, is quieter than it.

## Colour scheme

- Light or dark follows the reader's preference.
