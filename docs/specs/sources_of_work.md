# Sources of work

Linewright takes its work from what people already do in the repository.
Writing the spec is how the maintainer builds, and filing or labeling an issue
is how anyone asks for a fix; neither needs a separate request to Linewright.
For example, the maintainer merges a pull request that adds a section to the
spec, and Linewright picks it up and starts building it.

The maintainer can also describe work as automations in the repository. An
agent runs each automation when its trigger fires and files issues for the
pipeline to work, like any others.

## User Stories

1. As an operator, I want a merged change to the spec to start agents that bring
   the code in step with it, so that writing the spec is how I build.
2. As an operator, I want new issues triaged by an agent, so that reporters get
   an answer and clear issues become work without me.
3. As an operator, I want to hand any issue to the factory by labeling it, so
   that I decide what gets built.
4. As an operator, I want a spec change that is already implemented to produce
   no work, so that implementing by hand stays possible.
5. As an operator, I want agents to run on a schedule with instructions I write,
   so that recurring work happens without me.

## Spec changes

The spec is the tracked files matching the spec glob, and the spec as merged
is what gets built.

- **A spec change.** A pull request not Linewright's own, merged into the
  default branch, that touched a spec file.
- **Taking it up.** Linewright may take up a spec change that is not yet
  taken up, merged within the lookback window (`spec-lookback`) and marked
  for it.
- **Its issue.** Taking up a spec change opens its issue: "Spec change: <PR
  title>", linking the pull request and labeled `ready-for-agent`. The issue
  is the spec change's work from then on.
- **Taken up.** A spec change is taken up once an issue Linewright created,
  titled "Spec change: …", links it, or once it carries
  `linewright:designed`. Its issue is the first such issue.
- **A push to the default branch.** A push straight to the default branch is
  the maintainer's own change, outside the factory.

## Issues

Issues enter by their label, `needs-triage` for triage and `ready-for-agent`
for implementation, or by being assigned to Linewright, as under
[Repository](repository.md).

## Automations

Automations are a source of work too; see [Automations](automations.md).

## Polling

Linewright polls GitHub at an interval (`poll-interval`). With webhooks on, it
also reacts to GitHub's deliveries as they arrive; see
[Webhooks](webhooks.md).

- **The interval.** A poll starts one interval after the previous poll
  started.
- **One poll at a time.** When a poll takes longer than the interval, the next
  poll starts as soon as it ends.
- **What a poll reads.** What a poll reads from GitHub grows with Linewright's
  work. It does not grow with the rest of the repository.
- **Linewright's work.** The open items Linewright manages
  ([Managed items](repository.md#managed-items)), the items it is assigned to
  and the branches it pushed. Also the items closed or merged within
  `done-window` or `spec-lookback`.
- **Reaction time.** Without webhooks, Linewright starts to do what a change
  asks of it at most one interval and 10 s after the change. A change that
  waits, such as for a run to end or for a hold, counts from the end of its
  wait. This bound holds when Linewright's work is at most 100 items and
  branches, and GitHub answers each request within 1 s.
- **Beyond these conditions.** With more work, or with slower answers from
  GitHub, the 10 s grows in proportion to them.
