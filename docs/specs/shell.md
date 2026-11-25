# Shell

- **Layout.** Every screen but Sign in, Consent and Authorization error
  ([Screens](access.md#screens)) shows in the shell: a daisyUI `drawer`, with
  the navigation as its side and the content as its content.
  - **Expanded.** In the expanded window size class
    ([Layout](design_system.md#layout)), the navigation is beside the content,
    to its left (`lg:drawer-open`), as the sidebar or as the rail.
  - **Compact.** In the compact window size class, the navigation shows only
    when the reader opens it, and then it is the sidebar, over the content.
- **Navigation.** A column in `bg-base-200` that holds, from the top:
  - Repository: the avatar of the GitHub account that owns the repository, the
    size of the links' icons, then the repository's name. Both link to the
    repository on GitHub.
  - Links: a daisyUI `menu`, with daisyUI's padding, of links to
    [Board](board.md#board-1), [Runs](runs.md#runs-1),
    [Automations](automations.md#automations-1),
    [Benchmarks](benchmarks.md#benchmarks-2), [Server](operation.md#server)
    and [People](access.md#people-1). Each link is its icon, from the table
    below, then its name.
  - At its foot: the account, then the button that expands and collapses the
    navigation.

  | Link        | Icon           |
  | ----------- | -------------- |
  | Board       | `view-columns` |
  | Runs        | `command-line` |
  | Automations | `bolt`         |
  | Benchmarks  | `beaker`       |
  | Server      | `server`       |
  | People      | `users`        |

  - **The sidebar** is `w-64`. It has a padding of `px-2`, and `pt-10` at its
    top. The repository is `mx-5` from its sides, with `gap-2` under it.
  - **The rail** is `w-14`. Its padding is the `menu`'s alone, with `pt-10` at
    its top. The repository is `mx-5` from its sides. It shows the
    repository's avatar, the links' icons and the account's avatar, without
    their names. The name of each is in a daisyUI `tooltip` to its right,
    `tooltip-right`.
  - **Expanding and collapsing.** In the expanded window size class, a daisyUI
    `btn`, `btn-ghost` `btn-circle`, expands the rail to the sidebar, and
    collapses the sidebar to the rail. Its icon is `chevron-double-right` on
    the rail, and `chevron-double-right` mirrored on the sidebar. Its name is
    in a `tooltip-right`: "Expand navigation" on the rail, and "Collapse
    navigation" on the sidebar.
  - **Remembered.** The browser remembers whether the reader last chose the
    sidebar or the rail, on every page and across visits, for a year from
    their last choice. A page shows the reader's choice from its first paint.
    Until the reader chooses, the navigation is the sidebar.
- **The account.** A daisyUI `dropdown`, `dropdown-top`. Its button is a
  daisyUI `btn`, `btn-ghost`, that holds:
  - Avatar: the reader's [avatar](components.md#avatar).
  - Name: the reader's name, in `font-semibold`.
  - Role: the reader's role, under the name, in `text-xs`, in `secondary-text`.

  It opens a daisyUI `menu`, each entry its icon, then its label:
  - "Authorizations", with the icon `key`, which opens
    [Authorizations](access.md#authorizations).
  - "Sign out", with the icon `arrow-right-start-on-rectangle`.
- **The content.** A panel in `bg-base-100`, with a padding of `p-4`, or
  `p-10` in the expanded window size class. In the expanded window size class,
  it has `mt-2` above it and a `border-base-300` border along its top and its
  start. Its top start corner has `rounded-ss-4xl`, in the shape that CSS's
  `corner-start-start-shape: squircle` gives it. It holds, from the top:
  1. The top row: in the compact window size class, the button that opens the
     navigation, then the repository's name.
  2. The notices, while there are any.
  3. The breadcrumbs, where the screen has any, with the screen's buttons at
     their right.
  4. The screen, which fills the rest of the content. A screen without
     breadcrumbs has the screen's buttons at the right of its heading
     ([Heading](components.md#heading)).
- **Opening the navigation.** A daisyUI `btn`, `btn-ghost` `btn-square`
  `drawer-button`, with the icon `bars-3`. It shows only in the compact window
  size class.
- **The repository's name** at the top of the content is in `text-xl` and
  `font-light`, in `secondary-text`. Its row is at least as tall as a small
  button, `min-h-8`.
- **Breadcrumbs.** A daisyUI `breadcrumbs` that shows where the screen is in the
  UI, such as "Benchmarks › Design cuts › Benchmark run" on Benchmark run. The
  last crumb is the page's `h1`. Its row is at least as tall as a small
  button, `min-h-8`.
  - **The crumbs.** The first crumb is the screen's area, by its name in the
    navigation. Then comes a crumb for each thing in that area that the screen
    belongs to. The last crumb is the screen itself. A screen of one thing that
    belongs to no other thing, such as Run, has no breadcrumbs.
  - **Links.** The first crumb links to the area's screen in the navigation.
    Each other crumb but the last links to its screen.
  - **Names.** The last crumb shows the screen's name, such as "Benchmark
    run". On a screen of the navigation that lists things, it is "All" and what
    the screen lists, such as "All runs", or "All items" on Board. On Server,
    it is "Status". Each crumb between the first and the last shows the name in
    its screen's heading, such as "Design cuts".
  - **No such thing.** [No such thing](#no-such-thing) has no crumbs.
- **The screen's buttons.** The buttons that each screen gives for itself,
  such as "Cancel" on Benchmark run, in the order that the screen gives them.
- **Lost `serve`.** A screen that loses `serve` keeps what it last showed,
  shows "Lost `serve` 5 minutes ago; reconnecting.", with the time as a past
  time ([Times](design_system.md#times)), and reconnects on its own. The
  message is a daisyUI `alert`, `alert-warning`, with the icon `signal-slash`
  first.
- **Notices.** Every screen shows each hold on Linewright's work, such as a
  usage limit, and each problem that no item shows, while it lasts. Each is a
  [notice](components.md#notice) across the top of the page, in the status of
  the table below, with a link to the screen that tells more.

| Hold or problem                                          | Links to                                    | Shows as ([Status](design_system.md#status)) |
| -------------------------------------------------------- | ------------------------------------------- | -------------------------------------------- |
| a usage limit in force                                   | [Server](operation.md#server)               | paused                                       |
| a drain under way                                        | [Server](operation.md#server)               | paused                                       |
| a webhook delivery refused since the last one accepted   | [Server](operation.md#server)               | failed                                       |
| a poll that failed since the last that succeeded         | [Server](operation.md#server)               | failed                                       |
| an automation file that does not parse                   | [Automations](automations.md#automations-1) | failed                                       |
| a file of the repository's Linewright cannot use         | [Server](operation.md#server)               | failed                                       |
| a setup command with a failed latest build               | [Server](operation.md#server)               | failed                                       |
| a setup command with no environment yet, while it builds | [Server](operation.md#server)               | in flight                                    |
| a setup command with no environment yet, otherwise       | [Server](operation.md#server)               | paused                                       |
| an automation whose last run failed                      | that run's [Run](runs.md#run)               | failed                                       |

## No such thing

The screen that an address opens when nothing is at it: it names no screen,
or it names an item, run, benchmark, task or benchmark run that does not exist.

- **Title.** "Not found · Linewright".
- **Text.** "Not found · Linewright", as the page's `h1`, in `text-xl` and
  `font-semibold`.
