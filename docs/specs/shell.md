# Shell

- **Layout.** The navigation is to the left of the content. At the top of the
  content is the repository's name. Under it are the notices, while there are
  any, then the breadcrumbs, where the screen has any. The screen's buttons are
  at the right of the breadcrumbs, or of its heading on a screen without them
  ([Headings](design_system.md#headings)). A screen fills the rest of the
  content.
- **Navigation.** First the repository: the avatar of the GitHub account that
  owns it, then the repository's name. Both link to the repository on GitHub.
  Then links to [Board](board.md#board-1), [Runs](runs.md#runs-1),
  [Automations](automations.md#automations-1),
  [Benchmarks](benchmarks.md#benchmarks-2) and [Server](operation.md#server).
- **Breadcrumbs.** They show where the screen is in the UI, such as
  "Benchmarks › design-cuts › Benchmark run" on Benchmark run.
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
    last shows the name in its screen's heading, such as "design-cuts".
  - **No such thing.** A screen that shows "No such"
    ([Screen states](design_system.md#screen-states)) has the crumbs of the
    screen that its link names. A thing that Linewright cannot find has no
    crumb.
- **The screen's buttons.** The buttons that each screen gives for itself,
  such as "Cancel" on Benchmark run, in the order that the screen gives them.
- **Notices.** Every screen shows each hold on Linewright's work, such as a
  usage limit, and each problem that no item shows, while it lasts. A notice
  links to the screen that tells more.

| Hold or problem                                         | Links to                                    | Shows as ([Status](design_system.md#status)) |
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
