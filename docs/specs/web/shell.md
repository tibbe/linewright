# Shell on the web

- **Navigation** is a sidebar in the Card colour, `w-64` wide. It has a padding
  of `px-2`, and `pt-10` at its top. The repository is `mx-5` from its sides,
  with `gap-2` under it, and its avatar is the size of the links' icons,
  `size-4`. The links are a daisyUI `menu`, with daisyUI's padding. Where the
  page does not fit beside it, it is a rail of the links' icons and labels.
  Where the page does not fit beside the rail, it is a menu behind a button.
- **In the rail**, the repository's name is in a daisyUI `tooltip`.
- **A hold or a problem** is a daisyUI alert across the top of the page.
- **The name at the top** of the content is in `text-xl`, `font-light` and
  Muted text.
- **The row of the name at the top, and that of the breadcrumbs,** are each
  at least as tall as a small button, `min-h-8`.
- **The breadcrumbs** are daisyUI `breadcrumbs`, and the last crumb is the
  page's `h1`.
