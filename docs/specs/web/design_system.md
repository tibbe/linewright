# Web design system

The web UI follows daisyUI, with two themes of its own.

## Colour scheme

- The light theme, or the dark theme for a reader who prefers dark. Each is
  daisyUI's built-in theme of that name, with the values of the table at the
  end of this section in place of its own. The web UI draws in no colour but
  the roles below and the theme's fills, each fill with its content colour on
  it. Shadows are daisyUI's.

| Role       | daisyUI                       |
| ---------- | ----------------------------- |
| Page       | `base-100`                    |
| Card       | `base-200`                    |
| Border     | `base-300`                    |
| Muted text | `base-content` at 60% opacity |
| Text       | `base-content`                |

- **Secondary text** is in Muted text.

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
| paused    | Muted text |
| cancelled | `neutral`  |

- **A status's badge** is a small daisyUI `badge` in its colour.

## Cards

- **A card** is a daisyUI `card` in the Card colour, with daisyUI's
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

- **The dialog of "New"** holds a `fieldset`, with the dialog's title as its
  `fieldset-legend`. Each field is an `input`, a `select` or a
  `textarea` under its `label`, and the buttons are in the `modal-action`.
- **"New"**, and the button that makes the thing, are each a `btn-primary`.
- **The reason for a refusal** is a daisyUI `alert` in the `alert-error`
  style.

## Layout

- **The content** is a panel in the Page colour. From Tailwind's `lg` width,
  it has `mt-2` above it and a Border along its top and its start. Its top
  start corner has `rounded-ss-4xl`, in the shape that CSS's
  `corner-start-start-shape: squircle` gives it.
- **Spacing.** The content has a padding of `p-4`, or `p-10` from
  Tailwind's `lg` width. The parts of a screen are `gap-6` apart, and the parts
  of a card `gap-4`.
- **Text cut to fit** is shown whole in a daisyUI `tooltip`.
- **A name cut to fit** has its link to GitHub at the end of its last line.

## Filters

- **The filters** are a row of small daisyUI `input`s and `select`s, each
  with its label inside it as daisyUI's `label`.
- **A filter that searches** keeps the rows whose text holds what the reader
  types. It is a search `input` with a magnifier icon at its start, and it
  takes the spare width of the row.

## Tables

- **A table** is a daisyUI `table`. In a table whose first column names each
  row, a row's first cell is a `th`.
- **Its card** is a card ([Cards](#cards)), with the table's name as its
  title.
- **Its pages** are a daisyUI `join` of small buttons, and which rows show is
  in Muted text.
