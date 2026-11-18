# Navigation

The UI has many screens, in several areas, and a maintainer moves between them.
When Linewright's work is held, such as by the usage limit, they need to know at
once, whatever screen they are on. Every screen has the same chrome around it:
the navigation, which reaches the main screens, and a notice of each hold or
problem while it lasts.

For example, the usage limit is reached while a maintainer reads a
transcript. A notice of it shows on the Run screen they have open, and links
to Server, which says when the limit resets.

## User Stories

1. As a maintainer, I want to reach the main screens from any screen, so that
   I do not have to find my way back first.
2. As a maintainer, I want to see a hold on Linewright's work from any
   screen, so that I know why work stopped without looking for it.

## Chrome

- **Layout.** The chrome sits around the content: the navigation beside it,
  and the notices above it while there are any. A screen fills the content,
  but for one that shows over another, such as a confirmation.
- **Navigation.** The repository, as the avatar of the GitHub account that
  owns it beside its name, linking to it on GitHub; then links to
  [Board](board.md#board-1), [Runs](runs.md#runs-1),
  [Automations](automations.md#automations-1), [Suites](benchmarks.md#suites-1)
  and [Server](operation.md#server).
- **Notices.** A hold on Linewright's work, such as a usage limit, or a
  problem no item shows, is shown on every screen while it lasts, linking to
  the screen that tells more.

| Hold or problem                                                      | Links to                    |
| -------------------------------------------------------------------- | --------------------------- |
| a usage limit in force                                               | [Server](operation.md#server)           |
| a drain under way                                                    | [Server](operation.md#server)           |
| a webhook delivery refused since the last one accepted               | [Server](operation.md#server)           |
| a poll that failed since the last that succeeded                     | [Server](operation.md#server)           |
| an automation file that does not parse                               | [Automations](automations.md#automations-1) |
| a file of the repository's Linewright cannot use                  | [Server](operation.md#server)           |
| a setup command with no environment yet, or with a failed latest build | [Server](operation.md#server)           |
| an automation whose last run failed                                  | that run's [Run](runs.md#run)      |

## Out of scope

- Acting from the UI on `serve` or on the work, but for capturing a run and
  for suites and experiments.
