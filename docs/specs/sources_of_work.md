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
  default branch within the lookback window (default 7 days), that touched a
  spec file.
- **A push to the default branch.** A push straight to the default branch is
  the maintainer's own change, outside the factory.

## Issues

Issues enter by their label, `needs-triage` for triage and `ready-for-agent`
for implementation, or by being assigned to Linewright, as under
[Repository](repository.md).

## Automations

Automations are a source of work too; see [Automations](automations.md).

## Polling

Linewright polls, at an interval (default 60 s), and with webhooks on also
reacts to GitHub's deliveries as they arrive; see [Webhooks](webhooks.md).
