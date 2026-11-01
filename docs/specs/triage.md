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
5. As an operator, I want triage to report the facts and leave the change to
   the design, so that the design weighs every way to make it.

## Outcomes

A triage run ends by applying one of three status labels, and posts the
agent's comment on the issue.

| Label             | Means                                  |
| ----------------- | -------------------------------------- |
| `ready-for-agent` | an agent can work the issue            |
| `needs-info`      | the people asked owe a fact            |
| `ready-for-human` | a maintainer must decide               |

- **Category.** The category is the repository's issue type (Bug, Feature,
  Task) where the repository has issue types, and the `bug` or `enhancement`
  label otherwise.
- **Left as they are.** Triage leaves issues open and repository files as
  they are.

## Answers

The label says who must answer, and how.

- **`needs-info`.** The people asked answer in the thread. Linewright names
  them all in the comment. A reply is a comment from anyone other than
  Linewright posted after its question.
- **`ready-for-human`.** A maintainer answers with a label,
  `ready-for-agent` to implement as decided or `needs-triage` to triage
  again.

## The session

A triage session is sent `triage.md`, with these placeholders filled besides
those of every prompt ([Contract](runs.md#contract)).

| placeholder            | value                                                                   |
| ---------------------- | ----------------------------------------------------------------------- |
| `{{categories}}`       | the categories to choose from ([Outcomes](#outcomes))                   |
| `{{issue}}`            | the issue: its number, title, author, labels, body and comments         |

The session ends with a result that matches
[`results/triage.schema.json`](results/triage.schema.json).
