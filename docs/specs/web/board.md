# Board on the web

## Board

- **Columns.** The columns share the board's width, each at least 200px, and
  all six fit beside the navigation rail from 1440px wide.
- **A lane** is drawn as a card is ([Cards](design_system.md#cards)), and its
  head is a bar in the Card colour.
- **A card** is a daisyUI `card` drawn as a card is
  ([Cards](design_system.md#cards)), but in the Page colour. A card that
  shows a card status has a left edge in its colour and its icon before its
  line. A card in Done is faded.
- **On a lane or a card.** A lane or a card holds no daisyUI component that
  draws in `base-100`, `base-200` or `base-300`, such as a button, an input
  or an alert.
- **Person** is a `select`.

## Item

- **The facts** are in a card ([Cards](design_system.md#cards)), their labels
  in a column of their own.
