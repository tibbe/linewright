# Linewright on the web: design system

Linewright on the web follows daisyUI, with its components unchanged and
two themes of its own.

## Colour scheme

- The light theme, or the dark theme for a reader who prefers dark. Each is
  daisyUI's `light` or `dark` theme with its surfaces and text as below.

| Role   | Light     | Dark      |
| ------ | --------- | --------- |
| Page   | `#f7f8fa` | `#202226` |
| Lane   | `#eceef2` | `#191a1d` |
| Card   | `#ffffff` | `#2d3035` |
| Border | `#d8dbe2` | `#3d4047` |
| Text   | `#1c2024` | `#eceef2` |

## Status

| Outcome or state                                              | daisyUI colour |
| ------------------------------------------------------------- | -------------- |
| done, merged, green                                           | `success`      |
| failed, handed back, CI failing                               | `error`        |
| waiting for a person or for something                         | `warning`      |
| in flight, running                                            | `info`         |
| cancelled, interrupted, usage limit, paused or queued, closed | `neutral`      |

## Navigation

- **Navigation** is a sidebar at the left. Where the page does not fit
  beside it, it is a rail of the links' icons and labels; narrower still, a
  menu behind a button.
- **In the rail**, the repository's name is in a daisyUI `tooltip`.
- **A hold or a problem** is a daisyUI alert across the top of the page.

## Layout

- **Text cut to fit** is shown whole in a daisyUI `tooltip`.

## Board

- The columns share the board's width, each at least 200px, and all six fit
  beside the navigation rail from 1440px wide.
- **A lane** is a box in the Lane colour with a hairline Border and daisyUI's
  box radius. Its head is washed in its card status's colour, fading across
  the lane, with the card status's icon before its line.
- **A card** is a daisyUI `card` in the Card colour, with a hairline Border
  and a small shadow. A card that shows a card status has a left edge
  in its colour and its icon before its line. A card in Done is faded.

## Tables

- **A table** is a daisyUI `table`, its column heads pinned.
- **"By hand"** is a daisyUI `badge`.
