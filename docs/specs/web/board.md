# Board on the web

## Board

- **Columns.** The columns share the board's width, each at least 200px, and
  all six fit beside the navigation rail from 1440px wide.
- **The column heads** are a row in `bg-base-100`, `h-8` tall, with each
  column's name in `font-semibold`.
- **A lane** is drawn as a card is ([Cards](design_system.md#cards)), with
  `px-2` at its sides and `pb-2` at its foot. In a lane, the columns are
  `gap-2` apart, and so are the cards in a column.
- **A lane's head** is its top row, in `bg-base-200`, at least `h-10` tall,
  with `pt-2` above its text. Its text is in `font-semibold`, on one line, cut
  to fit. It is not a `card-title`, so it has no
  icon before its text.
- **A card** is a daisyUI `card`, `card-sm`, drawn as a card is
  ([Cards](design_system.md#cards)), but in `bg-base-100`. A card that shows a
  card status has an edge `3px` wide at its start in its colour, and its icon,
  in `base-content`, before its line. A card in Done is at `opacity-60`.
- **On a lane or a card.** A lane or a card holds no daisyUI component that
  draws in `base-100`, `base-200` or `base-300`, such as a button, an input
  or an alert.
- **Finished lanes** are under a daisyUI `divider`. Their fold, and each
  finished lane, is a daisyUI `collapse` with `collapse-arrow`.
- **Person** is a `select`.

## Item

- **The facts** are [facts](components.md#facts).
