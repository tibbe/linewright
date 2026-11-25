# Components

Linewright's own components: the parts of the web UI that daisyUI has no
component for ([Design system](design_system.md)).

## Content panel

- **The panel** is in `bg-base-100`.
- **In the expanded window size class** ([Layout](design_system.md#layout)),
  it has `mt-2` above it and a `border-base-300` border along its top and its
  start. Its top start corner has `rounded-ss-4xl`, in the shape that CSS's
  `corner-start-start-shape: squircle` gives it.

## Card with icon

- **A card with icon** is a card ([Cards](ui_patterns.md#cards)) whose
  `card-title` holds a `size-5` icon, then the title.

## Paused badge

- **A paused badge** is a daisyUI `badge`, `badge-outline`, in
  `base-content/60`.

## Facts

- **The card.** Facts are in a card ([Cards](ui_patterns.md#cards)), in a
  `grid` of rows `gap-3` apart, in `text-sm`.
- **A fact** is a row: its label at the start and its value at the end
  (`flex justify-between`).
- **A label** is words alone, with no icon or colour.
- **A status among facts** is in the value: a daisyUI `status` dot in its
  colour, then its icon, `size-[1em]`, then the value's words, `gap-1` apart. A
  paused dot is in `bg-base-content/60`.

## Transcript

- **A session's heading** is in `font-semibold`. It has `mt-4` above it, but
  for the first session's.
- **A session's settings** are in `text-sm`, in secondary text.
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
- **Durations.** An entry's duration is at the right edge of its line. A
  session's duration is at the right of its heading.
- **Hover.** An entry's line is in `bg-base-content/10` on hover, as an item
  of a daisyUI `menu` is.
- **What an entry opens to** is a block, such as a tool call's input and result
  or the full text of the brief. The block is in `bg-base-200`, with
  `rounded-box` corners and a padding of `px-3 py-2`. Its long lines wrap. A
  block taller than `max-h-[30rem]` stops there and scrolls on its own.
- **A sub-agent's work** has a line down the start of its entries,
  `border-s-2` in `base-content/10`, as a daisyUI `divider` is. The count of
  its entries is a daisyUI `badge`, `badge-sm`, with no colour or style class.
- **Thinking** is in secondary text.
- **"working"** is in secondary text, after a daisyUI `loading` in the
  `loading-dots` style, `loading-xs`.

## Inline code

- **Code in text,** such as `serve` in "Lost `serve`", is in `font-mono` at
  `text-[0.9em]`, on `bg-base-content/10`, with `rounded-sm` corners and a
  padding of `px-1`.

## Chart

- **The plot** is as wide as its card, in the proportions 480:260.
- **The axes** are lines along its left and its bottom, in
  `base-content/60`.
- **Grid lines** cross it at a pass rate of 50% and of 100%, in
  `base-content/10`, as a daisyUI `divider` is.
- **The scale** of the horizontal axis runs from 0 to 110% of the largest
  mean, so that no point is on its edge. Where the largest mean is 0, the scale
  runs to $1 for cost and to 1 second for time.
- **Ticks.** The vertical axis has labels at 0%, 50% and 100%. The horizontal
  axis has labels at 0, at half its scale and at its scale. The labels are in
  `text-xs`, in secondary text.
- **A point** is a `size-4` dot in `bg-primary`, with a `border-2` in
  `base-200`, the colour of its card. Its name is beside it in `text-xs`: to
  its right, or to its left when the point is in the right 40% of the plot.
- **"No results"** is in secondary text, at the centre of the plot, which
  then has no axes, grid lines or ticks.
