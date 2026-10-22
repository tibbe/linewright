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
- **In the rail**, the repository is its owner's GitHub avatar, with its
  name whole on hover and on keyboard focus.
- **A hold or a problem** is a daisyUI alert across the top of the page.

## Board

- The columns share the board's width, each at least 200px, and all six fit
  beside the navigation rail from 1440px wide.

## Tables

- **A table** is a daisyUI `table`, its column heads pinned.
- **"By hand"** is a daisyUI `badge`.
