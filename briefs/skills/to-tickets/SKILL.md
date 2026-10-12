---
name: to-tickets
description: Break a plan, spec, or the current conversation into a set of tracer-bullet tickets, each declaring its blocking edges, published to the configured tracker (edges as text in one file per ticket locally, or native blocking links on a real tracker).
disable-model-invocation: true
---

<!-- Forked from mattpocock/skills, skills/engineering/to-tickets (MIT, see
LICENSE). Made non-interactive for Linewright. -->

# To Tickets

You break one merged spec change into **tickets**: tracer-bullet vertical
slices, each declaring the tickets that **block** it. Your task holds
the pull request that changed the spec and its spec diff; the repository is
checked out at the default branch, with the change merged. You may read GitHub
but must not write to it: Linewright publishes the tickets in your result,
each as an issue labeled `ready-for-agent`, with your blocking edges as issue
dependencies, under one tracking issue. Change no files.

The decomposition is yours alone: nobody approves it before agents start on
it, so make the judgment calls the original skill would have put to a person
(granularity, edges, merges and splits) yourself.

## Process

### 1. Gather context

Read the pull request's title and body and the spec diff, then the spec as a
whole (the files matching the spec glob) so you know what the change means in
context. Read `GLOSSARY.md` if present: ticket titles and descriptions use its
vocabulary. Respect the architecture decisions in the area you're touching.

### 2. Explore the codebase

Find out what the code does today against what the changed spec asks.
**Behavior the code already has produces no ticket**: when everything the
change asks for is already there, the result has no tickets. Look for
opportunities to prefactor the code to make the implementation easier: "make
the change easy, then make the easy change".

### 3. Draft vertical slices

Break the remaining work into **tracer bullet** tickets.

- Each slice cuts a narrow but COMPLETE path through every layer (schema, API,
  UI, tests): vertical, NOT a horizontal slice of one layer.
- A completed slice is demoable or verifiable on its own.
- Each slice is sized so one fresh agent session finishes it in its smart
  zone, the part of its context window where it still works at full quality:
  implementing, testing, reviewing in fresh contexts and addressing the review.
  When in doubt, cut smaller.
- Any prefactoring comes first.

Give each ticket its **blocking edges**: the other tickets that must be merged
before it can start. A ticket with no blockers can start immediately. Keep the
edges honest: a ticket depends only on tickets that genuinely gate it, since
every edge serializes the work.

**Wide refactors are the exception to vertical slicing.** A wide refactor is
one mechanical change (rename a column, retype a shared symbol) whose blast
radius fans across the whole codebase, so no vertical slice can land green.
Sequence it as **expand–contract**: first expand (add the new form beside the
old so nothing breaks), then migrate the call sites in batches sized by blast
radius, each batch its own ticket blocked by the expand, then contract (delete
the old form) in a ticket blocked by every batch.

### 4. Review the breakdown yourself

Before answering, check the breakdown as the maintainer would have:

- Does the granularity feel right, neither too coarse for one session nor so
  fine that tickets can't be verified alone?
- Is every blocking edge real?
- Should any tickets be merged or split?

### 5. Write each ticket

Each ticket's body states, from the user's perspective:

```markdown
## What to build

The end-to-end behavior this ticket makes work, not a layer-by-layer list.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2
```

Leave blockers out of the body: they are edges in your result. Avoid file paths
and code snippets: they go stale. Exception: a snippet that encodes a decision
more precisely than prose (a state machine, schema or type shape), trimmed to
the decision-rich parts.

Your `comment` summarizes the breakdown for the spec change's pull request and
tracking issue: the tickets in order and why they are cut that way, or, with no
tickets, why the code already matches the spec.
