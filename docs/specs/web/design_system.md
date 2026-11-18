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

## Navigation

- **A screen** is a page, at the address below. Its link is its address.
- **A screen's title** is the page's `<title>`, which the browser shows on
  its tab.
- **A screen that shows over another**, such as a confirmation, is a daisyUI
  `modal`.

| Screen      | Address           |
| ----------- | ----------------- |
| Board       | `/`               |
| Item        | `/items/N`        |
| Runs        | `/runs`           |
| Run         | `/runs/ID`        |
| Automations | `/automations`    |
| Suites      | `/suites`         |
| Suite       | `/suites/NAME`    |
| Experiment  | `/experiments/ID` |
| Server      | `/server`         |

## Layout

- **Text cut to fit** is shown whole in a daisyUI `tooltip`.
- **A name cut to fit** has its link to GitHub at the end of its last line.

## Filters

- **The filters** are a row of small daisyUI `input`s and `select`s, each
  with its label inside it as daisyUI's `label`, at the left.

## Facts

- **The facts** are drawn as a card is, their labels in a column of their
  own.

## Tables

- **A table** is a daisyUI `table`, its column heads pinned.
