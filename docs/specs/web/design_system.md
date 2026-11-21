# Web design system

The web UI follows daisyUI, with two themes of its own.

## Components

- Each part of the web UI is the daisyUI component for it, where daisyUI has
  one, and looks as daisyUI draws it. Where a part differs from its component,
  the spec says how, in the classes of daisyUI and Tailwind.
- A part that daisyUI has no component for is one of Linewright's own
  [components](components.md). The spec defines each of them in the classes of
  daisyUI and Tailwind too.

## Colour scheme

- The light theme, or the dark theme for a reader who prefers dark. Each is
  daisyUI's built-in theme of that name, with the values of the table at the end
  of this section in place of its own. The web UI draws in no colour but
  `base-100`, `base-200`, `base-300`, `base-content` and the theme's fills, each
  fill with its content colour on what it holds. The exception is what daisyUI's
  components draw on their own, such as the shade over the content behind an
  open `drawer`. Shadows are daisyUI's.
- **Text** is in `base-content`, and **secondary text** in
  `text-base-content/60`.

| Variable | Light | Dark |
| --- | --- | --- |
| `--color-base-100` | `oklch(96% 0.008 68)` | `oklch(0.2046 0.007 257.8)` |
| `--color-base-200` | `oklch(97% 0.008 68)` | `oklch(0.2286 0.007 257.8)` |
| `--color-base-300` | `oklch(98% 0.008 68)` | `oklch(0.2486 0.007 257.8)` |
| `--color-base-content` | `oklch(21% 0.006 285.885)` | `oklch(97.807% 0.029 256.847)` |
| `--color-primary` | `oklch(46% 0.228 260.92)` | `oklch(46% 0.228 260.92)` |
| `--color-primary-content` | `oklch(93% 0.034 272.788)` | `oklch(96% 0.018 272.314)` |
| `--color-secondary` | `oklch(68% 0.163 184.32)` | `oklch(68% 0.163 184.32)` |
| `--color-secondary-content` | `oklch(98% 0.014 180.72)` | `oklch(94% 0.028 342.258)` |
| `--color-accent` | `oklch(0.3627 0.1049 260.92)` | `oklch(0.3627 0.1049 260.92)` |
| `--color-accent-content` | `oklch(0.7318 0.1049 260.92)` | `oklch(0.7318 0.1049 260.92)` |
| `--color-neutral` | `oklch(14% 0.052 265.87)` | `oklch(43% 0.046 261.32)` |
| `--color-neutral-content` | `oklch(92% 0.004 286.32)` | `oklch(92% 0.004 286.32)` |
| `--color-info` | `oklch(83% 0.094 211.43)` | `oklch(29% 0.066 243.157)` |
| `--color-info-content` | `oklch(29% 0.066 243.157)` | `oklch(83% 0.094 211.43)` |
| `--color-success` | `oklch(82% 0.092 163.21)` | `oklch(37% 0.077 168.94)` |
| `--color-success-content` | `oklch(37% 0.077 168.94)` | `oklch(82% 0.092 163.21)` |
| `--color-warning` | `oklch(83% 0.092 76.54)` | `oklch(41% 0.112 45.904)` |
| `--color-warning-content` | `oklch(41% 0.112 45.904)` | `oklch(83% 0.092 76.54)` |
| `--color-error` | `oklch(82% 0.112 24.87)` | `oklch(27% 0.105 12.094)` |
| `--color-error-content` | `oklch(27% 0.105 12.094)` | `oklch(82% 0.112 24.87)` |
| `--radius-selector` | `0.5rem` | `0.5rem` |
| `--radius-field` | `0.25rem` | `0.25rem` |
| `--radius-box` | `0.5rem` | `0.5rem` |
| `--size-selector` | `0.25rem` | `0.25rem` |
| `--size-field` | `0.25rem` | `0.25rem` |
| `--border` | `0.5px` | `0.5px` |
| `--depth` | `1` | `1` |
| `--noise` | `0` | `0` |

## Status

| Status    | Colour     |
| --------- | ---------- |
| done      | `success`  |
| failed    | `error`    |
| waiting   | `warning`  |
| in flight | `info`     |
| paused    | `base-content/60` |
| cancelled | `neutral`  |

- **A status's badge** is a daisyUI `badge`, `badge-sm`, with the icon first.
  A paused badge, whose colour is not a fill, is a `badge-outline` in
  `base-content/60`.

## Icons

- **The icons** are [Heroicons](https://heroicons.com) 2, in its 20 solid
  ("mini") style. The exceptions are the icons of an issue and of a pull
  request, which are GitHub's own, from
  [Octicons](https://primer.style/octicons). An icon is `size-5` before a
  `card-title`, `size-[1em]` in a `badge`, as daisyUI's badge with an icon has
  it, and `size-4` elsewhere. Where a daisyUI component draws an icon of its
  own, such as the arrow of a `select`, it keeps it.
- **Which icon.** Each part has the icon in the table below, and no other. A
  card has the icon of the area of the thing it holds, such as Runs for a
  run's heading or for a table of runs. The cards that the table names are the
  exceptions.

| Part                                       | Icon                        |
| ------------------------------------------ | --------------------------- |
| Board                                      | `view-columns`              |
| Runs                                       | `command-line`              |
| Automations                                | `bolt`                      |
| Benchmarks                                 | `beaker`                    |
| Server                                     | `server`                    |
| The card "Configurations"                  | `adjustments-horizontal`    |
| The card "Passes by task"                  | `table-cells`               |
| The card "Pass rate vs"                    | `chart-bar`                 |
| done                                       | `check-circle`              |
| failed                                     | `x-circle`                  |
| waiting                                    | `hand-raised`               |
| in flight                                  | `arrow-path`                |
| paused                                     | `pause-circle`              |
| cancelled                                  | `no-symbol`                 |
| "Add task"                                 | `plus`                      |
| "Cancel" that cancels work                 | `stop-circle`               |
| "Remove"                                   | `trash`                     |
| "Create"                                   | `plus`                      |
| "Run benchmark"                            | `play`                      |
| "Add a configuration"                      | `plus`                      |
| "Keep running"                             | `play`                      |
| "Cancel" that closes a dialog              | `x-mark`                    |
| The button that opens the navigation       | `bars-3`                    |
| The button that expands the navigation     | `chevron-double-right`      |
| The button that collapses the navigation   | `chevron-double-right`, mirrored |
| A chevron that points right                | `chevron-right`             |
| A chevron that points down                 | `chevron-down`              |
| A filter that searches                     | `magnifying-glass`          |
| A link to GitHub at the end of a name      | `arrow-top-right-on-square` |
| A card that is an issue                    | Octicons' `issue-opened`     |
| A card that is a pull request              | Octicons' `git-pull-request` |
| "Lost `serve`"                             | `signal-slash`              |
| "Jump to the latest entry"                 | `arrow-down`                |

## Cards

- **A card** is a daisyUI `card` in `bg-base-200`, with daisyUI's
  `shadow-xs` and no border.
- **A card's title** is its `card-title`, with an icon before it that stands
  for what the card holds.
- **A heading's name** is its card's `card-title`
  ([Headings](../design_system.md#headings)).

## Buttons

- **A button** is a small daisyUI `btn`, `btn-sm`. A button whose label is a
  verb has an icon before it that stands for what it does. A button on a table's
  row, and its "View" link, are each a `btn-ghost` `btn-xs`.

## Navigation

- **A screen** is a page, at the address below. Its link is its address.
- **A screen's title** is the page's `<title>`, which the browser shows on
  its tab.
- **A screen that shows over another**, such as a confirmation, is a daisyUI
  `modal` on a `<dialog>` that `showModal()` opens, with no `modal-backdrop`.
  This modal already closes as [Dialogs](../design_system.md#dialogs) says.
- **The button that goes ahead** in a confirmation is a `btn-error`.
- **A tab** is at the address of its screen, then `/` and the tab's name in
  lowercase, such as `/benchmarks/NAME/tasks`.

| Screen      | Address           |
| ----------- | ----------------- |
| Board       | `/`               |
| Item        | `/items/N`        |
| Runs        | `/runs`           |
| Run         | `/runs/ID`        |
| Automations | `/automations`    |
| Benchmarks      | `/benchmarks`         |
| Benchmark       | `/benchmarks/NAME`    |
| Benchmark run  | `/benchmark-runs/ID` |
| Server      | `/server`         |

## Making

- **The dialog in which the reader makes a thing** holds a `fieldset`, with the
  dialog's title as its `fieldset-legend`. Each field is an `input`, a `select`
  or a `textarea` under its `label`, and the buttons are in the `modal-action`.
- **"New"**, and the button that makes the thing, are each a `btn-primary`.
- **A field** is a daisyUI `validator`, with the browser's checks of its value,
  such as `required` or a `pattern`. "Optional" is a `label` under the field.
- **A field's refusal** is its `validator-hint`, and Linewright marks a field
  that it refuses `aria-invalid`.
- **A refusal that is not of one field** is a daisyUI `alert` in the
  `alert-error` style.
- **What was made**, on a screen that does not show it, is a daisyUI `alert` in
  the `alert-success` style.

## Layout

- **The content** is a [content panel](components.md#content-panel).
- **Spacing.** The content has a padding of `p-4`, or `p-10` from
  Tailwind's `lg` width. The parts of a screen are `gap-6` apart, and the parts
  of a card `gap-4`.
- **Text cut to fit** is shown whole in a daisyUI `tooltip`.
- **A name cut to fit** has its link to GitHub at the end of its last line.

## Filters

- **The filters** are a row of small daisyUI `input`s and `select`s, each
  with its label inside it as daisyUI's `label`.
- **A filter that searches** keeps the rows whose text holds what the reader
  types. It is a search `input` with its icon at its start, and it
  takes the spare width of the row.

## Tables

- **A table** is a daisyUI `table`. The cell that names a row is a `th`,
  wherever it sits. The head of the column of the buttons that open rows in
  place is an empty `th`.
- **Scrolling.** A table is in a wrapper with `overflow-x-auto`, as daisyUI's
  table is. As the table scrolls sideways, the top of its card and its filters
  stay in place.
- **Its card** is a card ([Cards](#cards)), with the table's name as its
  title.
- **Its pages** are a daisyUI `join` of small buttons, and which rows show is in
  secondary text.
