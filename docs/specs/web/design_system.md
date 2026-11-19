# Web design system

The web UI follows daisyUI, with two themes of its own.

## Colour scheme

- The light theme, or the dark theme for a reader who prefers dark. Each is
  daisyUI's `light` or `dark` theme with its surfaces and text as below, and
  the web UI draws in no colour but these and the theme's own fills, each with
  its content colour on it, and shadows are daisyUI's.
- **Surfaces and text** are steps of Radix Slate, but for a light Card, which
  is white.

| Role       | daisyUI        | Light: Slate step | Dark: Slate step |
| ---------- | -------------- | ----------------- | ---------------- |
| Page       | `base-100`     | 2                 | 3                |
| Lane       | `base-200`     | 3                 | 2                |
| Card       |                | white             | 5                |
| Border     | `base-300`     | 6                 | 6                |
| Muted text |                | 11                | 11               |
| Text       | `base-content` | 12                | 12               |

- **Secondary text** is in Muted text.

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
- **The button that makes the thing** is a `btn-primary`.
- **The reason for a refusal** is a daisyUI `alert` in the `alert-error`
  style.

## Layout

- **Text cut to fit** is shown whole in a daisyUI `tooltip`.
- **A name cut to fit** has its link to GitHub at the end of its last line.

## Filters

- **The filters** are a row of small daisyUI `input`s and `select`s, each
  with its label inside it as daisyUI's `label`.

## Tables

- **A table** is a daisyUI `table`. In a table whose first column names each
  row, a row's first cell is a `th`.
- **Its card** is drawn as a card on Board is, with the table's name as the
  bar's `card-title`, and no line between the bar and the column heads.
