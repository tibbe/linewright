---
name: triage
description: Move issues and external PRs through a state machine of triage roles, categorise, verify, grill if needed, and write agent-ready briefs.
---

# Triage

You triage one issue: verify its claim, categorize it and end in one of three
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

Finding facts is your job, never the reporter's or the maintainer's. A fact
you could observe by running something is not theirs to answer, and an issue
that just needs engineering investigation is not `needs-info`. Ask for what
only a person knows: what they saw, what they meant, a decision. Ask for a
fact only with a stated, specific reason you could not reach it yourself, and
only after driving it as far as it goes.

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
2. **Verify the claim.** For a bug, reproduce it yourself, with whatever your
   environment gives you: the repository and its history, the tools and
   credentials installed and configured, production where you have access.
   Check what you have rather than assume, and follow the repository's agent
   docs on what you can reach. If the reporter's input is out of reach (a
   private file, production data, a recording), synthesize the trigger from
   what the issue describes, with the same versions and settings as the code,
   tighten conditions, or instrument until it fires. For a feature request,
   check whether the behavior already exists. Note the result: confirmed (with
   the code path), or failed, with what you tried. A confirmed claim makes a
   much stronger brief.
3. **Find the cause.** For a reproduced bug, generate 3–5 ranked hypotheses
   before testing any of them. Each must be falsifiable: state the prediction
   it makes ("If <X> is the cause, then <changing Y> will make the bug
   disappear"). Test them one variable at a time. Note the hypothesis that
   turned out correct and the probe that showed it; one you could not test
   stays a hypothesis.
4. **Categorize.** Choose exactly one category from your task's list, from
   what you found: a bug when something is broken, otherwise the category for
   new features or improvements (or tasks, where the repository has that
   type).
5. **Decide.** Weigh the issue against the spec. Work the spec already asks for,
   or that it leaves to the implementer and that is unambiguous, is
   `ready-for-agent`. An issue that asks for behavior the spec rules out, or
   that needs the spec to change, is `ready-for-human`: the spec is the
   maintainer's. Where the request needs fleshing out and only the reporter can
   do it, `needs-info`; where only the maintainer can, `ready-for-human`.
   Facts come before decisions: while the reporter still owes a fact, choose
   `needs-info` and ask only for the missing facts, even when a maintainer's
   decision will follow, because that decision usually depends on the facts.
   A fact you could find yourself is not owed.
   Sharpen fuzzy terms against `GLOSSARY.md` in your comment; don't edit it.
6. **Write the comment** for the outcome. Don't add a disclaimer or say who
   wrote it: the comment is posted as Linewright.

## Writing the agent brief

The brief establishes the facts, so that the design that follows doesn't
have to do the research. The decisions, what to change and how to test it,
stay the design's. Issue readiness is separate from PR acceptance: the brief
requires no tests of an unwritten fix. Do not guess a solution, and do not
turn triage into a speculative implementation plan.

- **Durability over precision.** The issue may wait days or weeks while the
  code changes. Describe interfaces, types and behavioral contracts; name
  types, function signatures or config shapes to look for. Don't reference
  file paths or line numbers, and don't assume today's structure will stay.
- **Behavioral, not procedural.** Say what the system should do, not how to
  implement it. Good: "When a user runs `/triage` with no arguments, they
  should see a summary of issues needing attention". Bad: "Add a switch
  statement in the main handler function".

```markdown
## Agent Brief

**Category:** <category>
**Summary:** one line: what happens, and what should happen instead

**Current behavior:**
What happens now. For bugs, the broken behavior (and how you reproduced it).
For enhancements, the status quo the feature builds on.

**Expected behavior:**
What should happen instead, quoted from the reporter or the spec.

**Cause:**
For bugs, the hypothesis that turned out correct, and the probe that showed it.
```

## Needs-info notes

Questions must be specific and actionable, not "please provide more info",
and each says why only the person asked can answer it. Capture everything
already established so the work isn't lost, and list what you tried, so they
don't repeat it.

```markdown
## Triage Notes

**What we've established so far:**

- point 1

**What we tried:**

- what was tried, and why it fell short

**What we still need from you (@login, for each person asked):**

- question 1, and why only you can answer it
```

## Ready-for-human recommendations

Use the agent brief's structure where it fits, and add why the issue can't be
delegated: the judgment call, the decision or the spec change it needs. For a
duplicate or an already implemented feature, point at the other issue or the
code. For a request that conflicts with the spec or an earlier rejection,
quote it. End with your recommendation and the questions the maintainer must
answer, each answerable on its own.
