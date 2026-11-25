# UI patterns

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
  For example, "Remove" on Task and "Cancel" on Benchmark run need no
  more, but "Add task" on Run names the task.

## Pages

- **A screen** that is not a dialog ([Dialogs](#dialogs)) is a page, at the
  address that its section gives. Its link is its address.

## Titles

- **A screen's title** is the page's `<title>`, which the browser shows on
  its tab. It is the name in the screen's heading, then "Linewright", such as
  "Benchmark run 14 · Linewright". For a screen of the navigation, the name is
  the screen's name in the navigation. For a screen that shows without the
  shell, the name is the screen's name in its area's table of screens, such as
  "Sign in · Linewright".

## Headings

- **A heading.** A screen of one thing, such as a run, has a heading in a card
  of its own. The card is under the screen's breadcrumbs, or at its top where it
  has none. The heading holds the thing's name, then the badge of its state
  where the screen gives its state there. Under the name is the line that the
  screen gives, if any.
- **A heading's name** is its card's `card-title`.

## Cards

- **A card** is a daisyUI `card` in `bg-base-200`, with daisyUI's
  `shadow-xs` and no border.

## Buttons

- **A button** is a small daisyUI `btn`, `btn-sm`. A button whose label is a
  verb has an icon before it that stands for what it does. A button on a
  table's row, and its "View" link, are each a `btn-ghost` `btn-xs`.
- **Icons.** A button in the table below has its icon on every screen.

| Button                         | Icon            |
| ------------------------------ | --------------- |
| "Add task"                     | `plus`          |
| "Edit"                         | `pencil-square` |
| "Save"                         | `check`         |
| "Remove"                       | `trash`         |
| "Cancel" that cancels work     | `stop-circle`   |
| "Keep running"                 | `play`          |
| "Cancel" that closes a dialog  | `x-mark`        |

## Icons

- **Which icon.** A part has the icon that the spec gives it, and no other.

## Avatars

- **A person's avatar** is a daisyUI `avatar`, `size-8` and `rounded-full`.
  The letter in place of one is a daisyUI `avatar-placeholder` in
  `bg-neutral`.

## Screen states

- **Empty.** A list with nothing in it shows "No" and what it lists in its
  place, such as "No runs" or "No benchmarks", unless its screen shows it only
  while it holds something.
- **Lost.** A screen that loses `serve` keeps what it last showed, shows
  "Lost `serve` 5 minutes ago; reconnecting.", with the time as a past time
  ([Time](#time)), and reconnects on its own.
- **Nothing there.** A link can name an item, run, benchmark, task or benchmark
  run that does not exist. The screen it opens shows "No such" and the name, as
  the screens give it, such as "No such run 43/rework-9" or "No such item #812".
- **Absent.** A figure that is absent, such as a mean with nothing to
  count, shows as "–".
- **Loading.** A screen shows once it has all that it holds; it has no
  loading state.

## Filters

- **Where.** A screen's filters sit together above what they narrow, each
  labelled with what it filters on.
- **Their look.** The filters are a row of small daisyUI `input`s and
  `select`s, each with its label inside it as daisyUI's `label`.
- **Applying.** A filter applies as soon as it is chosen from a list, and
  one typed in when the reader presses Enter.
- **Unset.** An unset filter narrows nothing: a list reads "All" and what it
  lists, such as "All stages", and a field to type in is empty.
- **A filter that searches** keeps the rows whose text holds what the reader
  types. It is a search `input` with the icon `magnifying-glass` at its start,
  and it takes the spare width of the row.
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

- **Statuses.** A status is one of those in the table below. Each area says
  which status each of its states shows as.
- **Shown.** A status shows as its icon and its colour. Where a screen also
  names a status's state, the status's badge in the table below, in `badge-sm`,
  holds the icon, then the state's name. A status among a screen's facts is the
  exception ([Facts](components.md#facts)). For example, a running benchmark run
  shows as "running", with the icon of in flight, in its colour.
- **A badge's words.** The state's name in a badge gives no reason or
  detail: "no score", not why there is none.

| Status    | Icon           | Colour            | Badge                                              |
| --------- | -------------- | ----------------- | -------------------------------------------------- |
| done      | `check-circle` | `success`         | a daisyUI `badge`, `badge-success`                 |
| failed    | `x-circle`     | `error`           | a daisyUI `badge`, `badge-error`                   |
| waiting   | `hand-raised`  | `warning`         | a daisyUI `badge`, `badge-warning`                 |
| in flight | `arrow-path`   | `info`            | a daisyUI `badge`, `badge-info`                    |
| paused    | `pause-circle` | `base-content/60` | a [paused badge](components.md#paused-badge)       |
| cancelled | `no-symbol`    | `neutral`         | a daisyUI `badge`, `badge-neutral`                 |

## Tables

- **The card.** A table is in a card ([Cards](#cards)). The top of the card
  holds the table's name, as the card's title, at the left and the table's
  buttons at the right. The table's filters are in a row of their own under the
  top. Unless its screen names it, a table's name says what its rows are, such
  as "Runs".
- **A table in a tab.** A table that fills a tab has the tab's name.
- **No name shown.** The card of a table that fills a tab, or of the table
  that a screen of the navigation lists, shows no name: the tab or the
  breadcrumbs name it.
- **The table** is a daisyUI `table`.
- **Scrolling.** A table is in a wrapper with `overflow-x-auto`, as daisyUI's
  table is. As the table scrolls sideways, the top of its card and its filters
  stay in place.
- **A row's buttons.** A row has its buttons at its end, but for the button
  that opens it in place. No part of a row but a button or a link acts on a
  click.
  - **A row's screen.** A row that stands for a thing with a screen of its
    own has "View" at its end, a link to that screen. A screen reader reads
    the row's name after it, such as "View run 148/implement-1".
  - **Opening in place.** A row that opens in place, such as to rows under it,
    starts with a button that shows a chevron. The chevron points right
    (`chevron-right`) while the row is closed and down (`chevron-down`) while
    it is open. A screen reader reads the button as what it opens, such as
    "Show the tasks of opus-high". A row with nothing to open to has no button
    there, and its first cell keeps its place. The column of these buttons has
    an empty head, an empty `th`.
- **The column that names each row.** A column that names each row is the
  first column. In a table whose rows open in place, it is the second column,
  after the column of the buttons that open them. It has a head. Its cells are
  the rows' headers, each a `th`, in bold.
- **Alignment.** Amounts of money and durations align right. Every other
  column aligns left.
- A title in a table is on one line.
- **Pages.** A table with more than 25 rows shows them 25 at a time. Only the
  table's own rows count, not the rows that they open to.
  - **Under the table.** At the left is which rows show, of how many, such as
    "Showing 26–50 of 248 runs", in secondary text. At the right are
    "Previous", the numbers of the pages and "Next", a daisyUI `join` of small
    buttons. The numbers are those of the first page, the last and the two on
    each side of the reader's. "…" is in place of the numbers between them.
    "Previous" is disabled on the first page, and "Next" on the last.
  - **Links.** A screen's link carries its pages, as it carries its filters
    ([Filters](#filters)). Changing a filter goes back to the first page.
  - **Updates.** When the table gets rows or loses them, the reader stays on the
    page that they are on. A page that no longer exists shows as the last page.

## Tabs

- **Addresses.** A tab is at the address of its screen, then `/` and the
  tab's name in lowercase, such as `/benchmarks/ID/tasks`.
- **Links.** A screen's link carries the tab that it shows, as it carries its
  filters ([Filters](#filters)). A link with no tab opens on the tab that the
  screen opens on.

## Dialogs

- **Where.** A dialog opens over the whole page, the shell too
  ([Shell](shell.md)). It has no link and no breadcrumbs of its own.
- **Its look.** A dialog is a daisyUI `modal` on a `<dialog>` that
  `showModal()` opens, with no `modal-backdrop`.
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
  the button that asked, and is a `btn-error`. The other comes first and
  says "Cancel", or "Keep running" where the action cancels.
- **What was removed.** When the reader removes the thing that a screen
  shows, the screen of what held it opens. A message under its heading, a
  daisyUI `alert` in the `alert-success` style, says what was removed. The
  message stays until the reader leaves the screen.

## Making

- **New.** A table that the reader can add to has "New" with its screen's
  buttons ([Shell](shell.md)), or, for a table in a tab, in its card
  ([Tables](#tables)).
- **The dialog.** A button with which the reader starts to make a thing, such
  as "New", or "Add task" on Run, opens a dialog ([Dialogs](#dialogs)). The
  dialog shows each field with its label above it, then "Cancel" and the button
  that makes the thing.
- **The dialog's form.** The dialog holds a `fieldset`, with the dialog's
  title as its `fieldset-legend`. Each field is an `input`, a `select` or a
  `textarea` under its `label`, and the buttons are in the `modal-action`.
- **"New"**, and the button that makes the thing, are each a `btn-primary`.
- **Making.** When the reader clicks the button that makes the thing and
  Linewright makes it, the dialog closes.
- **What was made.** When a screen does not show what its dialog made, it
  shows a message under its heading, a daisyUI `alert` in the `alert-success`
  style. The message says what was made and links to it. It stays until the
  reader leaves the screen.
- **Required.** A field is required when Linewright cannot make the thing
  without it. Every other field is optional. An optional field shows "Optional"
  under it, as a `label`. A required field that is empty, or holds only white
  space, gets the message "Required."
- **A field** is a daisyUI `validator`, with the browser's checks of its value,
  such as `required` or a `pattern`.
- **A refusal.** When Linewright refuses, the dialog stays open. Each field that
  Linewright refuses shows the reason under it, as its `validator-hint`, and
  Linewright marks it `aria-invalid`. A refusal that is not of one field shows
  its reason above the buttons, a daisyUI `alert` in the `alert-error` style.
  The fields keep what the reader typed or chose.

## Editing

- **Edit.** A thing that the reader can change has "Edit". For a thing with a
  screen of its own, "Edit" is with that screen's buttons ([Shell](shell.md)),
  a plain `btn`. For any other thing, "Edit" is at the end of its row
  ([Tables](#tables)). A screen reader reads the row's name after it, such as
  "Edit opus-high".
- **The dialog.** "Edit" opens a dialog ([Dialogs](#dialogs)) titled "Edit"
  and what it changes, such as "Edit task". It is as the dialog in which the
  reader makes a thing ([Making](#making)), with "Save" in place of the button
  that makes the thing. It shows each field that the reader can change, with
  the thing's value in it.
- **Saving.** When the reader clicks "Save" and Linewright keeps the change,
  the dialog closes and the screen shows the change.
- **Required and refused.** A field is required when Linewright cannot keep
  the change without it. Linewright refuses a change as it refuses a thing in
  a dialog that makes it ([Making](#making)).
- **A thing that is gone.** When the thing does not exist, the dialog
  shows "No such" and the thing, as its screen does
  ([Screen states](#screen-states)), above the buttons.
- **At once.** When two readers change the same thing, the change saved last
  is the one that Linewright keeps.

## Type

- **Secondary text** is text that qualifies the text it sits beside, such as a
  card's line under its name, a fact's label or a transcript entry's time or
  duration. It is quieter than the text beside it
  ([Type](design_system.md#type)).

## Layout

- The UI is laid out for someone scanning a screen.
- **Spacing.** The parts of a screen are `gap-6` apart, and the parts of a
  card `gap-4`.
- Content wider than the screen scrolls sideways on its own, not the screen
  with it.
- What a screen shows on hover it also shows on keyboard focus, over the
  screen, until the pointer leaves it or focus moves. Text cut to fit is shown
  whole this way, in a daisyUI `tooltip`.
