# Design system

## Liveness

- A page shows `serve`'s state as it changes, without the reader reloading
  it.
- A page that loses `serve` keeps what it last showed, says since when, and
  reconnects on its own.

## Navigation

- Every page has the same bar.
- Every item or pull request named on a page links to its page and to
  GitHub.

## Time

- A past time is shown relative, such as "5 min ago", keeping up as time
  passes, with the absolute time in the reader's time zone shown on hover.
- A future time is shown absolute, in the reader's time zone.
- A duration reads the same on every page.

## Status

- An outcome or a state has an icon and a colour, those of GitHub's check
  state it maps to, and is never told by colour alone.

| Outcome or state                 | Check state |
| -------------------------------- | ----------- |
| done, merged, green              | success     |
| failed, handed back, CI failing  | failure     |
| in flight, running               | in progress |
| waiting for something            | pending     |
| cancelled, interrupted, usage limit, closed | cancelled |

## Colour scheme

- Light or dark follows the reader's preference.
