# Shell

- **Layout.** Every screen but Sign in, Consent and Authorization error
  ([Screens](access.md#screens)) shows in the shell. The navigation is to the
  left of the content. At the top of the
  content is the repository's name. Under it are the notices, while there are
  any, then the breadcrumbs, where the screen has any. The screen's buttons are
  at the right of the breadcrumbs, or of its heading on a screen without them
  ([Headings](ui_patterns.md#headings)). A screen fills the rest of the
  content.
  - **Compact.** In the compact window size class
    ([Layout](design_system.md#layout)), the navigation shows only when the
    reader opens it, and then it is the sidebar, over the content.
- **The content** is a [content panel](components.md#content-panel). It has
  a padding of `p-4`, or `p-10` in the expanded window size class.
- **Navigation.** First the repository: the avatar of the GitHub account that
  owns it, then the repository's name. Both link to the repository on GitHub.
  Then links to [Board](board.md#board-1), [Runs](runs.md#runs-1),
  [Automations](automations.md#automations-1),
  [Benchmarks](benchmarks.md#benchmarks-2), [Server](operation.md#server)
  and [People](access.md#people-1), each with its icon below. At the foot is
  the account.

  | Link        | Icon           |
  | ----------- | -------------- |
  | Board       | `view-columns` |
  | Runs        | `command-line` |
  | Automations | `bolt`         |
  | Benchmarks  | `beaker`       |
  | Server      | `server`       |
  | People      | `users`        |

  - **Its look.** The navigation is a daisyUI `drawer` in `bg-base-200`. In
    the expanded window size class, it is beside the content
    (`lg:drawer-open`), as the sidebar or as the rail.
  - **Opening it.** In the compact window size class, a `btn-ghost`
    `btn-square` `drawer-button` with the icon `bars-3`, at the start of the
    row of the name at the top, opens the navigation. It shows only in the
    compact window size class.
  - **The sidebar** is `w-64`. It has a padding of `px-2`, and `pt-10` at its
    top. The repository is `mx-5` from its sides, with `gap-2` under it, and
    its avatar is the size of the links' icons, `size-4`. The links are a
    daisyUI `menu`, with daisyUI's padding. Each link is its icon, then its
    name.
  - **The rail** is `w-14`. Its padding is the `menu`'s alone, with `pt-10` at
    its top. The repository is `mx-5` from its sides. It shows the
    repository's avatar and the links' icons, without their names. The name of
    each is in a daisyUI `tooltip` to its right, `tooltip-right`.
  - **Expanding and collapsing.** In the expanded window size class, a
    `btn-ghost` `btn-circle` at the foot of the navigation expands the rail to
    the sidebar, and collapses the sidebar to the rail. Its icon is
    `chevron-double-right` on the rail, and `chevron-double-right` mirrored on
    the sidebar. Its name is in a `tooltip-right`: "Expand navigation" on the
    rail, and "Collapse navigation" on the sidebar.
  - **Remembered.** The browser remembers whether the reader last chose the
    sidebar or the rail, on every page and across visits, for a year from
    their last choice. A page shows the reader's choice from its first paint.
    Until the reader chooses, the navigation is the sidebar.
- **The account.** The reader's avatar, then their name, with their role
  under it. It opens a menu: "Authorizations", which opens
  [Authorizations](access.md#authorizations), then "Sign out".
  - **Its look.** The account is a daisyUI `dropdown`, `dropdown-top`, above
    the button that expands and collapses the navigation. Its button is a
    `btn-ghost` that holds the avatar, then the name in `font-semibold` over
    the role in `text-xs`, in secondary text. Its menu is a daisyUI `menu`,
    each entry with its icon first: `key` for "Authorizations" and
    `arrow-right-start-on-rectangle` for "Sign out". On the rail, the button
    holds the avatar alone, and the name is in a `tooltip-right`.
- **The name at the top** of the content is in `text-xl` and `font-light`, in
  secondary text. Its row is at least as tall as a small button, `min-h-8`.
- **Breadcrumbs.** They show where the screen is in the UI, such as
  "Benchmarks › Design cuts › Benchmark run" on Benchmark run.
  - **The crumbs.** The first crumb is the screen's area, by its name in the
    navigation. Then comes a crumb for each thing in that area that the screen
    belongs to. The last crumb is the screen itself. A screen of one thing that
    belongs to no other thing, such as Run, has no breadcrumbs.
  - **Links.** The first crumb links to the area's screen in the navigation.
    Each other crumb but the last links to its screen.
  - **Names.** The last crumb shows the screen's name, such as "Benchmark
    run". On a screen of the navigation that lists things, it is "All" and what
    the screen lists, such as "All runs", or "All items" on Board. On Server,
    it is "Status". Each crumb between the first and the
    last shows the name in its screen's heading, such as "Design cuts".
  - **No such thing.** A screen that shows "No such"
    ([Screen states](ui_patterns.md#screen-states)) has the crumbs of the
    screen that its link names. A thing that Linewright cannot find has no
    crumb.
  - **Their look.** The breadcrumbs are daisyUI `breadcrumbs`, and the last
    crumb is the page's `h1`. Their row is at least as tall as a small button,
    `min-h-8`.
- **The screen's buttons.** The buttons that each screen gives for itself,
  such as "Cancel" on Benchmark run, in the order that the screen gives them.
- **"Lost `serve`"** ([Screen states](ui_patterns.md#screen-states)) is an
  `alert-warning`, with the icon `signal-slash` first.
- **Notices.** Every screen shows each hold on Linewright's work, such as a
  usage limit, and each problem that no item shows, while it lasts. A notice
  links to the screen that tells more.
  - **Their look.** A notice is a daisyUI `alert` across the top of the page,
    in the colour of its status, such as `alert-error` for failed. A status
    that has no daisyUI `alert` colour, such as paused or cancelled, gets the
    plain `alert`. Its status's icon is first, and its link is a daisyUI
    `link` at its end.

| Hold or problem                                         | Links to                                    | Shows as ([Status](ui_patterns.md#status)) |
| ------------------------------------------------------- | ------------------------------------------- | -------------------------------------------- |
| a usage limit in force                                  | [Server](operation.md#server)               | paused                                       |
| a drain under way                                       | [Server](operation.md#server)               | paused                                       |
| a webhook delivery refused since the last one accepted  | [Server](operation.md#server)               | failed                                       |
| a poll that failed since the last that succeeded        | [Server](operation.md#server)               | failed                                       |
| an automation file that does not parse                  | [Automations](automations.md#automations-1) | failed                                       |
| a file of the repository's Linewright cannot use        | [Server](operation.md#server)               | failed                                       |
| a setup command with a failed latest build              | [Server](operation.md#server)               | failed                                       |
| a setup command with no environment yet, while it builds | [Server](operation.md#server)              | in flight                                    |
| a setup command with no environment yet, otherwise      | [Server](operation.md#server)               | paused                                       |
| an automation whose last run failed                     | that run's [Run](runs.md#run)               | failed                                       |
