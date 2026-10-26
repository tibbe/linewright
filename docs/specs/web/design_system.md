# Linewright on the web: design system

Linewright on the web follows daisyUI, with its components and themes
unchanged.

## Colour scheme

- The `light` theme, or `dark` for a reader who prefers dark.

## Status

| Outcome or state                            | daisyUI colour |
| ------------------------------------------- | -------------- |
| done, merged, green                         | `success`      |
| failed, handed back, CI failing             | `error`        |
| in flight, running, waiting for something   | `warning`      |
| cancelled, interrupted, usage limit, closed | `neutral`      |

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
- **A lane** is a `base-200` box bordered `base-300`, with daisyUI's box
  radius.
- **A card** is a daisyUI `card` on `base-100`.

## Tables

- **A table** is a daisyUI `table`, its column heads pinned.
- **"By hand"** is a daisyUI `badge`.
