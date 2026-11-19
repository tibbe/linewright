# Shell

- **Layout.** The navigation is to the left of the content. At the top of
  the content are the notices, while there are any, then the breadcrumbs. A
  screen fills the rest of the content.
- **Navigation.** First the repository: the avatar of the GitHub account that
  owns it, then the repository's name. Both link to the repository on GitHub.
  Then links to [Board](board.md#board-1), [Runs](runs.md#runs-1),
  [Automations](automations.md#automations-1), [Suites](benchmarks.md#suites-1)
  and [Server](operation.md#server).
- **Breadcrumbs.** They show where the screen is in the UI, such as
  "Suites › design-cuts › Experiment 14" on Experiment.
  - **The crumbs.** The first crumb is the screen of the navigation for the
    screen's area. Then comes a crumb for each thing in that area that the
    screen belongs to. The last crumb is the screen itself.
  - **Links.** Each crumb but the last links to its screen.
  - **Names.** A crumb shows the name in its screen's heading, such as
    "Experiment 14", or "#812" and the item's title. The crumb of a screen of
    the navigation shows that screen's name in the navigation.
  - **No such thing.** A screen that shows "No such"
    ([Screen states](design_system.md#screen-states)) has the crumbs of the
    screen that its link names. The last crumb shows the name in the link.
    A thing that Linewright cannot find has no crumb.
- **Notices.** Every screen shows each hold on Linewright's work, such as a
  usage limit, and each problem that no item shows, while it lasts. A notice
  links to the screen that tells more.

| Hold or problem                                                        | Links to                                    |
| ---------------------------------------------------------------------- | ------------------------------------------- |
| a usage limit in force                                                 | [Server](operation.md#server)               |
| a drain under way                                                      | [Server](operation.md#server)               |
| a webhook delivery refused since the last one accepted                 | [Server](operation.md#server)               |
| a poll that failed since the last that succeeded                       | [Server](operation.md#server)               |
| an automation file that does not parse                                 | [Automations](automations.md#automations-1) |
| a file of the repository's Linewright cannot use                       | [Server](operation.md#server)               |
| a setup command with no environment yet, or with a failed latest build | [Server](operation.md#server)               |
| an automation whose last run failed                                    | that run's [Run](runs.md#run)               |
