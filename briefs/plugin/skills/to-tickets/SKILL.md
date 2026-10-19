---
name: to-tickets
description: Break a plan, spec, or the current conversation into a set of tracer-bullet tickets, each declaring its blocking edges, published to the configured tracker (edges as text in one file per ticket locally, or native blocking links on a real tracker).
disable-model-invocation: true
---

# To Tickets

Break the spec change in your task into a set of **tickets**: tracer-bullet vertical slices, each declaring the tickets that **block** it.

Your task holds the merged pull request that changed the spec and its spec diff; the repository is checked out at the default branch, with the change merged. You may read GitHub but must not write to it, and change no files: Linewright publishes the tickets in your result.

## Process

### 1. Gather context

Work from the spec change in your task. Read the whole spec, not only the diff, so you know what the change means in context.

### 2. Explore the codebase (optional)

If you have not already explored the codebase, do so to understand the current state of the code. Ticket titles and descriptions should use the project's domain glossary vocabulary, and respect ADRs in the area you're touching.

Look for opportunities to prefactor the code to make the implementation easier. "Make the change easy, then make the easy change."

### 3. Draft vertical slices

Break the work into **tracer bullet** tickets.

<vertical-slice-rules>

- Each slice cuts a narrow but COMPLETE path through every layer (schema, API, UI, tests): vertical, NOT a horizontal slice of one layer
- A completed slice is demoable or verifiable on its own
- Each slice is sized to fit in a single fresh context window
- Any prefactoring should be done first

</vertical-slice-rules>

Give each ticket its **blocking edges**: the other tickets that must complete before it can start. A ticket with no blockers can start immediately.

**Wide refactors are the exception to vertical slicing.** A **wide refactor** is one mechanical change (rename a column, retype a shared symbol) whose **blast radius** fans across the whole codebase, so a single edit breaks thousands of call sites at once and no vertical slice can land green. Don't force it into a tracer bullet; sequence it as **expand–contract**. First expand: add the new form beside the old so nothing breaks. Then migrate the call sites over in batches sized by blast radius (per package, per directory), each batch its own ticket blocked by the expand, keeping CI green batch to batch because the old form still exists. Finally contract: delete the old form once no caller remains, in a ticket blocked by every migrate batch. When even the batches can't stay green alone, keep the sequence but let them share an integration branch that all block a final integrate-and-verify ticket; green is promised only there.

### 4. Check the breakdown

Nobody approves the breakdown: agents start on the tickets as soon as Linewright publishes them, every unblocked ticket at once, each on its own branch. Write out the proposed breakdown as a numbered list. For each ticket, show:

- **Title**: short descriptive name
- **Blocked by**: which other tickets (if any) must complete first
- **What it delivers**: the end-to-end behaviour this ticket makes work
- **Touches**: the files it changes and the types and interfaces it adds

Apply these rules, in order:

1. **Shared work goes first.** When two tickets would add the same type or interface, move it into a prefactor ticket that blocks both.
2. **Tickets that change the same file run one after the other.** Merge them into one ticket; when the merged ticket would not fit a single fresh context window, make one block the other instead.
3. **Cut fewer tickets.** Put small work of the same shape in one ticket, and when unsure whether to split, keep it whole.

Ask yourself:

- Does the granularity feel right? (too coarse / too fine)
- Are the blocking edges correct: does each ticket only depend on tickets that genuinely gate it?
- Should any tickets be merged or split further?

Iterate until every two tickets that could run at the same time (neither blocks the other, directly or through other tickets) change different files and add different types.

### 5. Return the tickets in your result

Return the tickets in dependency order (blockers first), each with its blocking edges. Linewright publishes each as an issue labeled `ready-for-agent`, with its blocking edges as native blocking links, under a tracking issue for the spec change.

<issue-template>

## What to build

The end-to-end behaviour this ticket makes work, from the user's perspective, not layer-by-layer implementation.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2

</issue-template>

Avoid specific file paths or code snippets: they go stale fast. Exception: if a prototype produced a snippet that encodes a decision more precisely than prose can (state machine, reducer, schema, type shape), inline it and note briefly that it came from a prototype. Trim to the decision-rich parts, not a working demo, just the important bits.
