# Linewright on the web: design system

Linewright on the web follows daisyUI, with its components unchanged and
two themes of its own.

## Colour scheme

- The light theme, or the dark theme for a reader who prefers dark. Each is
  daisyUI's `light` or `dark` theme with its surfaces and text as below, and
  the web UI draws in no colour but these and the theme's own fills, each with
  its content colour on it: a wash is one of them made translucent behind
  what it washes, never text, and shadows are daisyUI's.
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

| Outcome or state                            | Colour     |
| ------------------------------------------- | ---------- |
| done, merged, green                         | `success`  |
| failed, handed back, CI failing             | `error`    |
| waiting for a person or for something       | `warning`  |
| in flight, running                          | `info`     |
| paused or queued                            | Muted text |
| cancelled, interrupted, usage limit, closed | `neutral`  |

## Navigation

- **Navigation** is a sidebar at the left. Where the page does not fit
  beside it, it is a rail of the links' icons and labels; narrower still, a
  menu behind a button.
- **In the rail**, the repository's name is in a daisyUI `tooltip`.
- **A hold or a problem** is a daisyUI alert across the top of the page.

## Layout

- **Text cut to fit** is shown whole in a daisyUI `tooltip`.
- **On a lane or a card**, no daisyUI component that draws in `base-100`,
  `base-200` or `base-300`, such as a button, an input or an alert.

## Board

- The columns share the board's width, each at least 200px, and all six fit
  beside the navigation rail from 1440px wide.
- **A lane** is a box in the Lane colour with a hairline Border and daisyUI's
  box radius. Its head is washed in its card status's colour, fading across
  the lane, with the card status's icon before its line.
- **A card** is a daisyUI `card` in the Card colour, with a hairline Border
  and a small shadow. A card that shows a card status has a left edge
  in its colour and its icon before its line. A card in Done is faded.

## Facts

- **The facts** are drawn as a card is, their labels in a column of their
  own.

## Tables

- **A table** is a daisyUI `table`, its column heads pinned.
- **"By hand"** is a daisyUI `badge`.
