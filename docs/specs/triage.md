# Triage

Triage is the first reading of a new issue: it establishes the facts, so that
the people who decide don't have to do the research. The decisions stay
theirs.

For example, a bug report without steps to reproduce gets a comment asking
the reporter for them. When the reporter answers in the thread, the issue is
triaged again, the bug reproduced, and the issue becomes a ticket with a
brief an implementer works from.

## User Stories

1. As an operator, I want new issues classified and their claims verified, so
   that only real, clear work reaches an implementer.
2. As an operator, I want reporters asked for what is missing and their answers
   picked up, so that I am not the go-between.
3. As an operator, I want an issue that needs my decision handed to me with a
   recommendation, so that my part is a decision, not research or
   bookkeeping.
4. As an operator, I want duplicates and already-fixed issues flagged rather
   than closed, so that no reporter is turned away by a machine.

## Outcomes

Triage establishes every fact it can reach itself: the agent reads the
issue and its comments, the codebase and the spec, categorizes, verifies the
claim (reproduces a bug, runs the tests, checks whether the behavior already
exists) and ends in one of three states.

| State             | Comment                                                                                                                                                                 |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ready-for-agent` | a triage brief: summary, current and desired behavior, acceptance criteria, out of scope                                                                                |
| `needs-info`      | addressed to the people asked: what is established, what was tried, what is still needed                                                                                |
| `ready-for-human` | a recommendation: the questions a maintainer must answer, or why the issue is not for an agent (duplicate, already implemented, needs a spec change, should be rejected) |

- **Facts before decisions.** A fact only a person knows, what they saw or
  meant, makes the outcome `needs-info`, asking only for it, even if a
  maintainer's decision will follow, because the decision usually depends on
  the facts.
- **Category.** The category is the repository's issue type (Bug, Feature,
  Task) where the repository has issue types, and the `bug` or `enhancement`
  label otherwise.
- **Left as they are.** Triage leaves issues open and repository files as
  they are.

## Answers

The label says who must answer, and how.

- **`needs-info`.** The people asked answer in the thread. Linewright assigns
  the people it asks, those GitHub lets it assign, and names them all in the
  comment. A reply, a comment from anyone other than Linewright posted after
  its question, puts the issue back to `needs-triage`.
- **`ready-for-human`.** A maintainer answers with a label,
  `ready-for-agent` to implement as decided or `needs-triage` to triage
  again.

The `triage` states of the issue lifecycle; both are labels.

| State          | Event                                | Next              | Effect |
| -------------- | ------------------------------------ | ----------------- | ------ |
| —              | `needs-triage` applied               | `needs-triage`    |        |
| `needs-triage` | a person owes a fact                 | `needs-info`      |        |
| `needs-info`   | a person comments                    | `needs-triage`    |        |
| `needs-triage` | triage ends in `ready-for-agent`     | `ready-for-agent` |        |
| `needs-triage` | triage ends in `ready-for-human`     | `ready-for-human` |        |
