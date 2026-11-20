# Shell on the web

- **Navigation** is a daisyUI `drawer` in `bg-base-200`. From Tailwind's `lg`
  width, it is beside the content (`lg:drawer-open`), as the sidebar or
  as the rail. Below `lg`, it shows only when the reader opens it, and then
  it is the sidebar, over the content. The button that opens it is a
  `btn-ghost` `btn-square` `drawer-button`, at the start of the row of the
  name at the top. It shows only below `lg`.
- **The sidebar** is `w-64`. It has a padding of `px-2`, and `pt-10` at its
  top. The repository is `mx-5` from its sides, with `gap-2` under it, and its
  avatar is the size of the links' icons, `size-4`. The links are a daisyUI
  `menu`, with daisyUI's padding. Each link is its icon, then its name.
- **The rail** is `w-14`, with the sidebar's paddings and margins, as daisyUI's
  drawer is when it shows only icons. It shows the repository's avatar and the
  links' icons, without their names. The name of each is in a daisyUI
  `tooltip` to its right, `tooltip-right`.
- **Expanding and collapsing.** From `lg`, a `btn-ghost` `btn-circle` at the
  foot of the navigation expands the rail to the sidebar, and collapses the
  sidebar to the rail. Its name is in a `tooltip-right`: "Expand navigation"
  on the rail, and "Collapse navigation" on the sidebar.
- **Remembered.** The browser remembers whether the reader last chose the
  sidebar or the rail, on every page and across visits, for a year from their
  last choice. A page shows the reader's choice from its first paint. Until the
  reader chooses, the navigation is the sidebar.
- **A notice** is a daisyUI `alert` across the top of the page, in the colour of
  its status, or daisyUI's plain `alert` for paused. Its status's icon is first,
  and its link is a daisyUI `link` at its end.
- **"Lost `serve`"** ([Screen states](../design_system.md#screen-states)) is an
  `alert-warning`, with its icon first.
- **The name at the top** of the content is in `text-xl` and `font-light`, in
  secondary text.
- **The row of the name at the top, and that of the breadcrumbs,** are each
  at least as tall as a small button, `min-h-8`.
- **The breadcrumbs** are daisyUI `breadcrumbs`, and the last crumb is the
  page's `h1`.
