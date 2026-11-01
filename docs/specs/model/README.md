# A model of the spec

A model of Linewright's spec in [Quint](https://quint-lang.org), checked
exhaustively within the bounds below. It covers how an item's state is derived,
what Linewright does in each state (the Working an item table, first row that
holds), how people steer it, and what the board shows: `repository.md` (Labels,
Assignment, State, Working an item, Steering, Managed items, Writes),
`design.md` (Designing, Cutting), `sources_of_work.md` (Spec changes),
`triage.md` (Outcomes, Answers), `pull_requests.md` (The pull request,
Working a pull request, CI, Rework, Keeping up to date, Merging), `runs.md`
(Ending) and `design_system.md` (States, The board, Cards).

Re-run it on every change to those files, and change it in the same commit.

## Files

- `facts.qnt`: what GitHub records about an item.
- `linewright.qnt`: the spec's rules, each definition named after the spec's
  term and quoting the clause it encodes; the properties.
- `pipeline.qnt`, `cut.qnt`, `spec_change.qnt`: the three models, each a set
  of items, bounds and starts, with its scenarios.
- `pull_request.qnt`: a model of its own of one pull request worked by the
  Working a pull request table, with its properties.
- `package.json`, `package-lock.json`: the pinned Quint.

## How it runs

Linewright makes one GitHub write a step, and may stop (a crash or a kill)
between any two: every series of writes, such as a hand-back, a triage
outcome, a cut or a discard, is that many steps. People act at any step: they
add and remove status labels, `linewright:designed` and the required label;
assign and unassign Linewright and themselves; close and reopen issues; close
and merge pull requests; add and remove sub-issues; and comment. GitHub closes
the issues a merged pull request closes, some steps after the merge. A run's
session is one step, which ends it with one of the outcomes its stage allows.

What an item's history holds, such as "since last opened or given a status
label", "was unassigned from", "added to sub-issues by whom" or "Linewright's
last act was letting go", is a fact of the item, set by the write that makes
it so.

## The models and their bounds

| Model         | Items                                                                                                                                                           | Starts                                                                                                          |
| ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `pipeline`    | an issue; an issue that may block it; Linewright's pull request for it; a person's pull request that closes it, which a person may open                          | the issue labeled `needs-triage`, `ready-for-agent` or nothing, marked or not, blocked or not; `require-label` and `--auto-merge` each on or off |
| `cut`         | an issue labeled `ready-for-agent`; two tickets its cut may create; Linewright's pull request for the first; a person's pull request that closes the first; a person's issue and Linewright's spec gap issue, which people may add as sub-issues | `require-label` on or off; `--auto-merge` on                                                                    |
| `spec_change` | a merged spec change; the issue Linewright opens for it; a spec gap issue that references it, filed by its issue's design or before it was taken up; one ticket; the pull requests for the issue and the ticket | in the lookback window or not; the gap filed before or not; `require-label` on or off; `--auto-merge` on        |

In every model people act twice a trace and Linewright stops once. In `cut`
people label, assign, close and merge the issue, the first ticket and the two
pull requests; in `spec_change`, the spec change, its issue and the gap issue.
A cut takes the ticket slots still free, so a cut after a discard is smaller.
An issue is built as it is only where it has a pull request slot; the
second ticket in `cut` has none, so its implement run can only hand it back.
Holds (the usage limit, the drain, `max-runs`), CI, reviews and automations
are not modelled in these three; `pull_request` models CI and reviews.

`pull_request` is one open pull request Linewright opened and is assigned
to, a draft with no checks yet, `--auto-merge` on or off, mergeable by the
repository's methods or not. CI and GitHub act on their own: checks appear
within the grace period or it passes, they finish green or failed or time
out, and GitHub decides whether it allows the merge. Checks may appear once
after the grace period too. A run ends with a push, without one, or unable to
finish, and every run brings the branch up to date. A merge GitHub allowed
may still fail. People act twice a trace: they comment, approve, push, move
the default branch, block or unblock the merge, unassign and assign
Linewright, mark it a draft, and close or merge it. Linewright stops once.

## Assumptions

Readings the spec leaves open, each named where the model uses it.

- **A1.** The comment that records an outcome is the first write of its
  series.
- **A2.** "One writer" is one series of writes at a time across all items;
  runs are in flight on several items at once.
- **A3.** `ready-for-agent` asks for both the design and the implement run, so
  a person changing `linewright:designed` during either cancels neither.
- **A4.** A cancel takes effect at a later step of Linewright's, not with the
  person's change, so a run can be in flight on an item that no longer asks
  for it (the State table's "has a run in flight" row). A run applying its
  result is not cancelled, and is in flight until its last write.
- **A5.** A run ends by the Working an item rows for its stage, whatever the
  item's state is by then.
- **A6.** A series of writes goes on to its end whatever people do to the item
  meanwhile; nothing in the spec says otherwise.
- **A7.** "It claims a pull request it opened" on an issue's terms: unassigned,
  marked, and not claimed again once Linewright was unassigned until the
  required label is applied again.
- **A8.** Of the Working an item rows, only the first (a cut left behind)
  holds for a closed issue: every other row's State is an open item's.
- **A9.** A ticket cut from a spec change's issue, and a spec gap issue its
  design files, reference the spec change.
- **A10.** A person's comment on a `needs-info` issue Linewright has worked is
  a reply.
- **A11.** In "Asking for its work", "open" applies to both clauses.
- **A12.** A person who adds a status label may leave the old one, for
  Linewright to remove.
- **A13.** A cut writes, in this order: the spec gap issue; each ticket,
  created with its labels as a sub-issue in one write; each blocked-by link;
  the comment; `linewright:designed`.
- **A14.** A run whose result a stop interrupted decides the same when run
  again.
- **A15.** The Ready line "unassigned" holds when Linewright was unassigned
  since the issue's status label or the required label was last applied.
- **A16.** A claim is one write, the assignment; `linewright:stalled` is
  cleared by a write of its own once Linewright is assigned. Cleared first, in
  the same series, a person's relabel between the two writes has Linewright
  claim an item whose state no longer asks for a claim.

## The properties

| Property                     | Definitions                                                       | Checked by                                            |
| ---------------------------- | ----------------------------------------------------------------- | ----------------------------------------------------- |
| S1 Derivation                | `S1_derivation`; a witness scenario per row                        | TLC, invariant; `quint test`                          |
| S2 Words and columns         | `S2_wordsAndColumns`, `S2_tables`                                  | TLC, invariant; `quint test`                          |
| S3 No card waits on nobody   | `S3_noCardWaitsOnNobody`                                           | TLC, invariant                                        |
| S4 Work follows state        | `S4_workFollowsState`, judged at each claim and run start          | TLC, invariant                                        |
| S5 Crash-safety              | `S5_crashSafety`                                                   | TLC, temporal, weak fairness of Linewright and GitHub |
| S6 Progress                  | `S6_progress`; `S6_restsWhereQuiet`; `S6_noFlap`                    | TLC, temporal; invariants                             |
| S7 People's items are theirs | `S7_peoplesItemsAreTheirs`, judged at each close and detach        | TLC, invariant                                        |
| S8 Regressions               | the scenarios named after them                                     | `quint test`                                          |
| S9 Motivating issues         | the scenarios named after the issues                               | `quint test`                                          |

- **S5.** If Linewright stops between two writes and people leave the item
  alone, it finishes as the whole series would have, or the item comes to
  rest as S6 says. The end a series would have reached is the item's state
  and, while it is open, its `linewright:designed` and Linewright's
  assignment, after the rest of the series. A person's action after the stop
  discharges it.
- **S6.** Left alone, every item comes to rest: done; waiting on a person (an
  answer, a decision, a review or merge, or a label or assignment after they
  stopped Linewright); or outside Linewright's work. A tracking issue comes
  to rest with its tickets. Its temporal half is that Linewright eventually
  has nothing more to do, for good; its safety half, `S6_restsWhereQuiet`, is
  that wherever it has nothing more to do, every managed item rests. Nothing
  flaps: "never repeats a write it has undone" counts from the last person's
  action or run result on the way; its ghost makes a run four times as
  large, so `S6_noFlap` is checked from `initFlaps`.
- **In `pull_request`.** S1 is a witness for each row, the state where it is
  the first that holds (`row0Unreached` to `row13Unreached`, each violated);
  S5 is `crashSafety`; S6 is `progress`, `restsWhereQuiet` (merged or closed;
  handed back or unassigned; or ready for review and waiting on the
  maintainer, a person who holds its merge or what GitHub requires) and
  `noFlap` (with people, CI and GitHub idle, Linewright starts no run twice).
  `attemptsBounded` holds fix-ci runs to `ci-attempts` and failed merges to
  two a round.

## Running it

`scripts/check model` installs the pinned tools into
`${XDG_CACHE_HOME:-$HOME/.cache}/linewright` and runs, for each model, its
scenarios and three TLC runs: `invariants` (S1 to S4, S6's resting places and
S7), `S6_noFlap` from `initFlaps`, and `S5_crashSafety` with `S6_progress`; and
for `pull_request`, its invariants, its temporal properties and a witness for
each row. It takes about two hours, most of it the flap and liveness runs of
`cut` and `spec_change` (4 to 33 million states each), so it is not among the
checks `scripts/check` runs without names, nor in CI. To check one property by
hand, with the tools it installed (`QUINT_HOME` at `quint-home` there, its Java
21 first on the `PATH`), run Quint from a scratch directory, since Apalache
writes its output where it runs:

```sh
model="$PWD/docs/specs/model"
cd "$(mktemp -d)"
quint test "$model/cut.qnt"
quint verify --backend=tlc --invariant=S7_peoplesItemsAreTheirs "$model/cut.qnt"
quint verify --backend=tlc --temporal=S5_crashSafety "$model/pipeline.qnt"
quint run --mbt --invariant=S3_noCardWaitsOnNobody "$model/pipeline.qnt"
```
