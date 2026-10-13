# Sources of work

Linewright takes its work from what people already do in the repository.
Writing the spec is how the maintainer builds, and filing or labeling an issue
is how anyone asks for a fix; neither needs a separate request to Linewright.
For example, the maintainer merges a pull request that adds a section to the
spec, and Linewright picks it up and starts building it.

The maintainer can also describe work as workflows in the repository. An
agent runs each workflow when its trigger fires and files issues for the
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

## Decisions

- The spec is the tracked files matching the spec glob.
- A **spec change** is a pull request that was merged into the default
  branch, touched at least one spec file, was merged within the lookback
  window (default 7 days), has no `linewright:spec-processed` label and,
  with `require-label` set, carries the required label, applied before or
  after the merge. PRs authored by the machine user are skipped. The spec as
  merged is what gets built; a push straight to the default branch is the
  maintainer's own change, outside the factory.
- Issues enter in two ways: with `needs-triage`, which asks for triage, and
  with `ready-for-agent`, which asks for implementation once the issue is
  unclaimed and unblocked.
- Workflows are a source of work too; see [Workflows](workflows.md).
- Linewright polls, at an interval (default 60 s), and with webhooks on
  also reacts to GitHub's deliveries as they arrive; see
  [Webhooks](webhooks.md).
