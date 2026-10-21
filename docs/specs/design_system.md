# Design system

## Liveness

- A page shows `serve`'s state as it changes, without the reader reloading
  it.
- A page that loses `serve` keeps what it last showed, says since when, and
  reconnects on its own.
- What the reader opened or closed stays so through updates, until they
  open the page again.

## Navigation

- Every page has the same navigation.
- A hold on Linewright's work, such as a usage limit, or a problem no item
  shows, is shown on every page while it lasts, linking to the page that
  tells more.
- Every item or pull request named on a page links to its page and to
  GitHub.

## Time

- A past time is shown relative, such as "5 min ago", keeping up as time
  passes, with the absolute time in the reader's time zone shown on hover.
- A future time is shown absolute, in the reader's time zone.
- A duration reads the same on every page.

## Status

- An outcome or a state has an icon and a colour, shared by those that mean
  the same, and is never told by colour alone.

## Board

- As the reader scrolls, the column heads stay at the top, with the head of
  the lane they are in under them.
- A card's title is on at most two lines, and whole on hover.

## Colour scheme

- Light or dark follows the reader's preference.
