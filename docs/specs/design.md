# Design

A design decides the structure of the code before any of it is written, so
that a change is built to a structure chosen for it rather than patched into
the one that is there.

For example, a bug report says that exporting a large report fails. The
design finds that the exporter holds the whole report in memory, a structure
that makes the failure likely for any large report, and chooses one that
streams it. The work is cut in two: a ticket that makes the exporter stream,
then a ticket that fixes the export, and the bug becomes the tracking issue
of both.

## User Stories

1. As an operator, I want the structure of the code chosen before any of it
   is written, so that a change is built the best way known rather than
   patched into whatever structure is there.
2. As an operator, I want a fix to remove the kind of problem and not only the
   instance, so that issues like it stop coming.
3. As an operator, I want work cut into tickets an agent session finishes
   within its smart zone, so that the work is done by agents at their
   sharpest; that each PR is small enough to review is a welcome side effect.
4. As an operator, I want an issue too big for one session cut like a spec
   change, so that size never sends it back to me.
5. As an operator, I want tickets ordered by dependency and worked in that
   order, so that each builds on merged work.
6. As an operator, I want one place that shows a spec change's progress, so that
   I know when it is done.

## Designing

Every item is designed before it is built, by its first run; a ticket is
created designed, with the design of the item it was cut from, and is never
designed on its own.

1. An agent designs the item from what it asks: a spec change's PR and spec
   diff, or an issue and its comments.
2. It cuts the work into tickets, as under [Cutting](#cutting).
3. Linewright records the design in a comment on the tracking issue or the
   issue: the design and, where the work was cut, the tickets in order and
   why they are cut that way. A spec change's PR gets the tickets in order
   and why, with a link to the tracking issue.
4. The item is labeled `linewright:designed`. An item carrying the label is
   not designed again.

- **1a. Nothing to build.** The code already does what the item asks. A spec
  change is labeled `linewright:designed`, and its PR gets a comment saying
  why; an issue is handed back, as triage hands back an issue already
  implemented.
- **1b. The design cannot be made.** The item is handed back. A spec gap that
  does not block the design is filed as under [Runs](runs.md).
- **Discarding a cut.** An item cut into tickets that is without
  `linewright:designed`, because its cut was interrupted before the label was
  set or a person removed it, is designed again, reading what the cut left.
  First its open tickets and their pull requests are closed as not planned
  with a comment, and so is a spec change's tracking issue.
- **Responsibility.** The design is Linewright's: it runs from a spec change
  or a claimed issue to the ready PRs with no human step, and a wrong design
  or set of tickets from a good spec is a defect in Linewright.

## Cutting

The design cuts work into tickets where it needs cutting, in the order they
are to be built.

- **Tickets.** Each ticket is a sub-issue of its tracking issue, carries
  `ready-for-agent` and `linewright:designed`, and is linked to the tickets
  it is blocked by with GitHub's issue dependencies.
- **A spec change.** Its work is always at least one ticket,
  under a tracking issue Linewright creates: "Spec change: <PR title>",
  linking the PR and listing the tickets. A spec change's tracking issue is
  the open issue Linewright created that references the pull request.
- **An issue.** One that fits a session is built as it is; one that is cut
  becomes its tickets' tracking issue.
- **The tracking issue** carries no status label.

## The session

A design session is sent `design.md`, with these placeholders filled besides
those of every prompt ([Contract](runs.md#contract)).

| placeholder      | value                                                                                                                       |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `{{kind}}`       | `spec change` or `issue`, what is designed                                                                                  |
| `{{item}}`       | a spec change's number, title, author, body and spec diff, or the issue and its comments        |
| `{{filed_gaps}}` | the spec gaps already filed for the item, or nothing                                             |
| `{{cutting}}`    | how the work is cut, for a spec change or an issue ([Cutting](#cutting))                                                       |

The session ends with a result that matches
[`results/design-spec-change.schema.json`](results/design-spec-change.schema.json)
for a spec change, or
[`results/design-issue.schema.json`](results/design-issue.schema.json) for an
issue.

## Out of scope

- Building a spec change without tickets, as an issue is built.
