---
name: triage
description: Move issues and external PRs through a state machine of triage roles, categorise, verify, grill if needed, and write agent-ready briefs.
disable-model-invocation: true
---

<!-- Forked from mattpocock/skills, skills/engineering/triage (MIT, see
LICENSE). Made non-interactive for Linewright. -->

# Triage

You triage one issue: categorize it, verify its claim and end in one of three
outcomes, with the comment that explains it. The issue, its comments and the
categories you may choose from are in your task. You may read GitHub but
must not write to it: everything you want posted goes in your result, and
Linewright posts it. Leave the issue open and the repository's files as they
are; you may build and run the code to verify a claim.

## Outcomes

- **`ready-for-agent`**: the issue is clear, within the spec, and an agent can
  implement it without a person deciding anything. Your comment is an agent
  brief (see _Writing the agent brief_).
- **`needs-info`**: someone must answer before anyone can act, usually the
  reporter. Name the people you ask in `ask`, by login. Linewright assigns them,
  and a reply from anyone in the thread sends the issue back to triage. Your
  comment is triage notes addressed to them by @-mention (see
  _Needs-info notes_).
- **`ready-for-human`**: the maintainer must act or decide. Your comment is a
  recommendation: the questions the maintainer must answer, or why the issue is
  not for an agent: a duplicate, already implemented, needs a spec change, or
  should be rejected. You flag; you never close or reject.

## Process

1. **Gather context.** Read the issue and every comment. Parse earlier triage
   notes so you don't re-ask resolved questions; when the reporter or the
   maintainer has answered, build on the answers. Read `GLOSSARY.md` if the
   repository has one and use its terms. Read the spec (the files matching the
   spec glob in your task) and respect the architecture decisions in the area.
   Explore the codebase. Run two checks:
   - **Redundancy**: search for an existing implementation of the requested
     behavior by domain concept, not just by the request's wording, and note
     where you looked. If it already exists, the outcome is `ready-for-human`
     with a pointer to where it lives.
   - **Prior rejection**: read `.out-of-scope/*.md` if present. A match by
     concept (not keyword) means `ready-for-human`, quoting the earlier
     decision and its reason.
2. **Categorize.** Choose exactly one category from your task's list: a bug
   when something is broken, otherwise the category for new features or
   improvements (or tasks, where the repository has that type).
3. **Verify the claim.** For a bug, reproduce it from the reporter's steps; run
   the relevant tests or commands. For a feature request, check whether the
   behavior already exists. Note the result: confirmed (with the code path),
   failed, or not enough detail to try (a strong `needs-info` signal). A
   confirmed claim makes a much stronger brief.
4. **Decide.** Weigh the issue against the spec. Work the spec already asks for,
   or that it leaves to the implementer and that is unambiguous, is
   `ready-for-agent`. An issue that asks for behavior the spec rules out, or
   that needs the spec to change, is `ready-for-human`: the spec is the
   maintainer's. Where the request needs fleshing out and only the reporter can
   do it, `needs-info`; where only the maintainer can, `ready-for-human`.
   Facts come before decisions: while the reporter still owes a fact, choose
   `needs-info` and ask only for the missing facts, even when a maintainer's
   decision will follow, because that decision usually depends on the facts.
   Sharpen fuzzy terms against `GLOSSARY.md` in your comment; don't edit it.
5. **Write the comment** for the outcome. Don't add a disclaimer or say who
   wrote it: the comment is posted as Linewright.

## Writing the agent brief

The brief is the authoritative specification an agent will work from; the
issue body and discussion are context. It states **what the agent should
do**.

- **Durability over precision.** The issue may wait days or weeks while the
  code changes. Describe interfaces, types and behavioral contracts; name
  types, function signatures or config shapes to look for. Don't reference
  file paths or line numbers, and don't assume today's structure will stay.
- **Behavioral, not procedural.** Say what the system should do, not how to
  implement it. Good: "The `SkillConfig` type should accept an optional
  `schedule` field of type `CronExpression`". Bad: "Open src/types/skill.ts and
  add a field on line 42".
- **Complete acceptance criteria.** Concrete, testable, each independently
  verifiable. Good: "Running `triage list --json` emits valid JSON for both
  success and error cases". Bad: "Triage should work correctly".
- **Explicit scope boundaries.** State what is out of scope, so the agent
  doesn't gold-plate or assume adjacent features.

```markdown
## Agent Brief

**Category:** <category>
**Summary:** one-line description of what needs to happen

**Current behavior:**
What happens now. For bugs, the broken behavior (and how you reproduced it).
For enhancements, the status quo the feature builds on.

**Desired behavior:**
What should happen once the work is done, including edge cases and errors.

**Key interfaces:**
- `TypeName`: what needs to change and why
- `functionName()` return type: what it returns now vs what it should return
- Config shape: any new options

**Acceptance criteria:**
- [ ] Specific, testable criterion 1
- [ ] Specific, testable criterion 2

**Out of scope:**
- What should NOT be changed or addressed here
```

## Needs-info notes

Questions must be specific and actionable, not "please provide more info".
Capture everything already established so the work isn't lost.

```markdown
## Triage Notes

**What we've established so far:**

- point 1

**What we still need from you (@login, for each person asked):**

- question 1
```

## Ready-for-human recommendations

Use the agent brief's structure where it fits, and add why the issue can't be
delegated: the judgment call, the decision or the spec change it needs. For a
duplicate or an already implemented feature, point at the other issue or the
code. For a request that conflicts with the spec or an earlier rejection,
quote it. End with your recommendation and the questions the maintainer must
answer, each answerable on its own.
