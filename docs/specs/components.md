# Components

Linewright's own components that more than one screen uses, built from
daisyUI's components and tokens ([Design system](design_system.md)). A slot
holds text, or another component that the use names.

## Card

A daisyUI `card` in `bg-base-200`, with `shadow-xs`:

- Content: what the card holds, and how it is laid out.

## Card with icon

A [card](#card) whose content is a daisyUI `card-body`, which holds its
`card-title`, then the body. The `card-title` holds the icon, then the title:

- Icon: `size-5`.
- Title: the card's title.
- Body: what the card holds under its title.

## Heading

A [card](#card) that names one thing, such as a run. Its content is a daisyUI
`card-body` that holds two parts side by side, aligned at their tops, `gap-4`
apart, and wraps where they do not fit:

- **The name.** It takes the width that the buttons leave, and holds two
  parts, one under the other, `gap-1` apart:
  - A row that holds the `card-title` and the state side by side, centred on
    one another, `gap-3` apart, and wraps where they do not fit. The
    `card-title` holds the icon, then the title.
  - The line.
- **The buttons.** Side by side, `gap-2` apart.

Each use gives these slots:

- Icon: the thing's icon, `size-5`.
- Title: the thing's name.
- State (optional): a [status badge](#status-badge) of its state.
- Line (optional): in `secondary-text`.
- Buttons (optional): the screen's [buttons](#button).

## Button

A small daisyUI `btn`, `btn-sm`, that holds the icon, then the label:

- Icon (optional): the button's icon.
- Label: the button's label.

## Row button

A daisyUI `btn`, `btn-ghost` `btn-xs`, that holds the slots of a
[button](#button), and one more:

- Name: what a screen reader reads after the label, such as "opus-high" in
  "Edit opus-high".

## Avatar

A person's avatar: a daisyUI `avatar` that holds a circle, `size-8` and
`rounded-full`:

- Picture (optional): the person's picture. Without one, the circle is a
  daisyUI `avatar-placeholder` in `bg-neutral`, which holds a letter in place
  of the picture.

## Item name

An item, on one line: its title, then its number, side by side, aligned on
their baselines, `gap-1` apart:

- Title (optional): the item's title, where Linewright has read the item, a
  daisyUI `link`, `link-hover`, to the item's [Item](board.md#item).
- Number: the item's number, in `secondary-text` and `font-normal`, a daisyUI
  `link`, `link-hover`, to the item on GitHub.

## Item name in text

An item as words in a run of text: its title, then a space, then its number.
The title and the number wrap between words with the text around them. The
title and the number are as in an [item name](#item-name).

## Status badge

A status, with the name of its state: a daisyUI `badge`, `badge-sm`, in the
class of its status in the table below. It holds the icon, then the label:

- Icon: the status's icon ([Status](design_system.md#status)).
- Label: the state's name, with no reason or detail.

| Status    | Class                                 |
| --------- | ------------------------------------- |
| done      | `badge-success`                       |
| failed    | `badge-error`                         |
| waiting   | `badge-warning`                       |
| in flight | `badge-info`                          |
| paused    | `badge-outline`, in `base-content/60` |
| cancelled | `badge-neutral`                       |

## Status text

A status, in a line of text: its dot, its icon, then its text, side by side,
aligned on their baselines, `gap-1` apart:

- Dot: a daisyUI `status` in the status's colour
  ([Status](design_system.md#status)).
- Icon: the status's icon, `size-[1em]`, in its colour.
- Text: the words that the use gives, such as the name of a state.

## Notice

A daisyUI `alert` in the colour of its status, such as `alert-error` for
failed. A status that has no daisyUI `alert` colour, such as paused or
cancelled, gets the plain `alert`. It holds the icon, the text, then the link:

- Icon: its status's icon ([Status](design_system.md#status)).
- Text: what the notice says.
- Link (optional): a daisyUI `link`.

## Facts

Facts about a thing: a `grid` of rows, `gap-3` apart, in `text-sm`. Each row
is a fact, and holds its label at its start and its value at its end, at least
`gap-6` apart (`flex justify-between gap-6`):

- Label: the fact's label, in `secondary-text`.
- Value: text, or a component that the use names, such as a
  [status text](#status-text). Each line of the value is a box of its own,
  aligned at the end.

## Stats

Facts about a thing: a daisyUI `stats` that is also a [card](#card). Each
fact is a daisyUI `stat`, which holds the label, then the value:

- Label: the fact's label, as the `stat-title`.
- Value: the fact's value, as the `stat-value`, in `text-xl` and
  `font-semibold`.

## Table

A table in a [card](#card), with `pt-4`. The card holds these parts, one
under the other, `gap-4` apart:

- **Top.** A row, with `px-4`, that holds the name at its start and the buttons
  at its end, centred on one another, and wraps where they do not fit. The name
  is the card's `card-title`: the icon, then the name. The buttons are side by
  side, `gap-2` apart. A table whose top shows no name and has no buttons has no
  top.
- **Filters.** The table's [filters](#filters), with `px-4`.
- **The table.** A daisyUI `table`, in a wrapper with `overflow-x-auto`, as
  daisyUI's table is.
- **Pages.** Under the table, while it has more than 25 rows.

Each use gives these slots:

- Icon (optional): `size-5`.
- Name: the table's name, such as "Runs", and whether the top shows it.
  Where the top does not show it, a screen reader reads it as the card's name.
- Buttons (optional): each a [button](#button).
- Filters (optional).
- Columns: each with its head, what it holds, and whether it aligns left or
  right.
- Rows: what each row stands for, in their order.
- A row's screen (optional): for a row that stands for a thing with a screen
  of its own, that screen.
- A row's buttons (optional).

How the table works:

- **Scrolling.** As the table scrolls sideways, the top of its card and its
  filters stay in place.
- **The column that names each row** is the first column. It has a head. Its
  cells are the rows' headers, each a `th`, in bold.
- **Titles.** A title in a table is on one line.
- **Empty.** With no rows, the card holds "No" and what the rows are, in the
  plural, such as "No tasks", in place of the `table`, in a paragraph with
  `px-4 pb-4`.
- **A row's buttons.** A row has its buttons at its end, each a
  [row button](#row-button) whose name is the row's name. No part of a row but
  a button or a link acts on a click.
- **A row's screen.** A row that stands for a thing with a screen of its own
  has "View" at its end, a link to that screen, drawn as a
  [row button](#row-button) whose name is the row's name, such as "View run
  148/implement-1".
- **Pages.** A table with more than 25 rows shows them 25 at a time.
  - **Under the table.** At the left is which rows show, of how many, such as
    "Showing 26–50 of 248 runs", in `secondary-text`. At the right are
    "Previous", the numbers of the pages and "Next", a daisyUI `join` of small
    buttons, `btn-sm`. The numbers are those of the first page, the last page,
    and the two pages on each side of the reader's page. "…" is in place of the
    numbers between them. "Previous" is disabled on the first page, and "Next"
    on the last.
  - **Links.** A screen's link carries its table's page, as it carries its
    filters ([Filters](#filters)). Changing a filter goes back to the first
    page.
  - **Updates.** When the table gets rows or loses them, the reader stays on
    the page that they are on. A page that no longer exists shows as the last
    page.

## Filters

The filters of what a screen shows, above what they narrow: a row of small
daisyUI `input`s and `select`s, side by side, centred on one another, `gap-2`
apart, which wraps where they do not fit. Each holds its label inside it, in a
daisyUI `label`. Each use gives, for each filter:

- Label: what it filters on.
- Kind: an `input` to type in, or a `select` to choose from a list.
- What it keeps.

How the filters work:

- **Applying.** A filter applies as soon as it is chosen from a list, and one
  typed in when the reader presses Enter.
- **Unset.** An unset filter narrows nothing: a list reads "All" and what it
  lists, such as "All stages", and a field to type in is empty.
- **Narrowing.** What the filters leave out is gone from the screen and from
  its counts. A count that its screen says is of everything, however it is
  filtered, still counts it.
- **Links.** A screen's link carries its filters: a reader who opens the
  screen again, or follows its link, gets the same filters. A link to the
  screen from another screen carries the filters that it names, and no
  others.
- **Kept.** An update never changes a filter.
- **Opened and closed.** Changing a filter leaves what the reader opened or
  closed as it was.

## Dialog

A daisyUI `modal` on a `<dialog>` that `showModal()` opens, with no
`modal-backdrop`. It opens over the whole page, the shell too
([Shell](shell.md)).

- **Closing.** A click outside a dialog does nothing. Escape closes it, as the
  button that backs out of it does.

## Form dialog

A [dialog](#dialog) in which the reader makes or changes a thing. It holds a
`fieldset`:

- `fieldset-legend`: the dialog's title.
- Fields: each an `input`, a `select` or a `textarea` under its `label`.
- `modal-action`: "Cancel", a [button](#button) with the icon `x-mark`, which
  backs out, then the dialog's button, a [button](#button), `btn-primary`.

How it works:

- **Done.** When the reader clicks the dialog's button and Linewright makes
  the thing or keeps the change, the dialog closes.
- **Required.** A field is required when Linewright cannot make the thing or
  keep the change without it. Every other field is optional. An optional field
  shows "Optional" under it, as a `label`. A required field that is empty, or
  holds only white space, gets the message "Required."
- **A field** is a daisyUI `validator`, with the browser's checks of its value,
  such as `required` or a `pattern`.
- **A refusal.** When Linewright refuses, the dialog stays open. Each field that
  Linewright refuses shows the reason under it, as its `validator-hint`, and
  Linewright marks it `aria-invalid`. A refusal that is not of one field shows
  its reason above the buttons, a daisyUI `alert` in the `alert-error` style.
  The fields keep what the reader typed or chose.

## Edit dialog

A [form dialog](#form-dialog) in which the reader changes a thing, opened by
"Edit".

- Title: "Edit" and what it changes, such as "Edit task".
- Fields: each field of the thing that the reader can change, with the thing's
  value in it.
- The dialog's button: "Save", with the icon `check`.

How it works:

- **A thing that is gone.** When the thing does not exist, the dialog shows
  "No such", then what it edits and its ID, such as "No such task 7", above
  the buttons.

## Confirmation

A [dialog](#dialog) that asks before an action goes ahead. It holds:

- Question: names what the action acts on and says what follows.
- The button that turns it down: its label.
- The button that goes ahead: its label, which is that of the button that
  asked.

Its `modal-action` holds first the button that turns it down and backs out, a
[button](#button). Then it holds the button that goes ahead, a
[button](#button), `btn-error`, with the icon of the button that asked.

How it works:

- **The answer.** The reader confirms, and the action goes ahead, or turns it
  down, and nothing changes.

## Success message

What the reader made or removed: a daisyUI `alert` in the `alert-success`
style. It stays until the reader leaves the screen.

- Text: what it says.

## Card page

A screen without the shell, in a [card](#card) in the middle of the page.

- **The page** has the card in its middle:
  `flex min-h-screen items-center justify-center p-4`.
- **The card** has `w-full max-w-100 p-12`. It holds, from the top:
  - Heading: in `text-3xl` and `font-bold`, centred, with `mb-2` under it.
  - The repository's name: in `secondary-text`, centred, with `mb-6` under it.
  - Body: the rest of what the screen holds.

## Transcript

A run's transcripts ([Transcripts](runs.md#transcripts)), each under its
session's heading.

- **A session's heading** is in `font-semibold`. It has `mt-4` above it, but
  for the first session's. It holds the session's number and when it started,
  such as "session 1: started 5 minutes ago", then, at its right, how long it
  has run. This time does not include a wait for the usage limit to reset.
  Where Linewright does not know when the session ended, such as when a
  restart interrupted its run, the time goes up to its last entry.
- **A session's settings** are under its heading, in `text-sm`, in
  `secondary-text`: the session's model and effort, which open on click to where
  its settings and brief files came from.
- **Entries** are in `font-mono` and `text-sm`.
- **Lines.** Each entry's line starts with a column for a chevron. An entry that
  opens shows a chevron there: it points right (`chevron-right`) while the entry
  is closed and down (`chevron-down`) while it is open. A chevron is `size-4`.
  In other entries the column is empty. A session's settings and brief files
  open with the same chevron.
- **A status in an entry.** `running` and `interrupted`, which an entry shows
  as a status ([Transcripts](runs.md#transcripts)), are the status's icon,
  `size-[1em]`, in its colour, then the word, `gap-1` apart.
- **Indents.** The text of an entry starts at the same place on each of its
  lines. What the entry opens to starts there too.
- **Durations.** An entry's duration is at the right edge of its line, in
  `secondary-text`.
- **Hover.** An entry's line is in `bg-base-content/10` on hover, as an item
  of a daisyUI `menu` is.
- **What an entry opens to** is a block, such as a tool call's input and result
  or the full text of the brief. The block is in `bg-base-200`, with
  `rounded-box` corners and a padding of `px-3 py-2`. Its long lines wrap. A
  block taller than `max-h-[30rem]` stops there and scrolls on its own.
- **A sub-agent's work** has a line down the start of its entries,
  `border-s-2` in `base-content/10`, as a daisyUI `divider` is. The count of
  its entries is a daisyUI `badge`, `badge-sm`, with no colour or style class.
- **Thinking** is in `secondary-text`.
- **"working"** is in `secondary-text`, after a daisyUI `loading` in the
  `loading-dots` style, `loading-xs`.

How it works:

- **Views.** A transcript never shows what the agent was told other than its
  prompts. Verbose, it is in full ([Transcripts](runs.md#transcripts)), and a
  session's settings are open too.
- **Prompts.** The brief and each follow-up are entries, each shown as its
  first line.
- **Durations.** Each entry that runs shows how long it ran, and the time
  goes up while the entry runs. If its session ends first, the time stops
  there ([Transcripts](runs.md#transcripts)).
  The entry's account does not repeat the time.
- **Working.** While a followed session's turn goes on and no entry runs, the
  session's last line says "working" and how long the turn has gone on. The
  time goes up as the turn goes on. Between turns, there is no such line. While
  the session waits for the usage limit to reset, the line says so and until
  when instead, such as "waiting for the usage limit, until 14:30".
- **Folding.** An entry opens on click to what its line leaves out. For a tool
  call, that is its full input and result, and its sub-agent's work. For the
  clone, it is its error, and for the maintenance command, its output. For the
  brief or a follow-up, it is its full text. For a "thought" entry, it is the
  thinking. A "thought" entry with no thinking
  text does not open. An entry that failed starts open. A sub-agent's work
  starts folded to the line of the call that started it, with the count of its
  entries.

## Inline code

Code in a line of text, which the spec writes in backticks, such as a
command's name: in `font-mono` at `text-[0.9em]`, on `bg-base-content/10`,
with `rounded-sm` corners and a padding of `px-1`.
