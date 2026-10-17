# Linewright

The vocabulary of a software factory that turns a repository's spec and
issues into software. The specs in `docs/` and the code use these terms as
defined here.

## Language

### Roles

**Operator**:
The person who runs Linewright.
_Avoid_: admin, user

**Maintainer**:
The person who owns the repository.
_Avoid_: owner, reviewer, human

**Reporter**:
A GitHub user who files an issue.
_Avoid_: customer, author

**Agent**:
A program that works on code on its own, such as Claude Code.
_Avoid_: bot, worker

**Session**:
An agent's work in one context, from its first prompt to its last.
_Avoid_: conversation, task

**Transcript**:
The record of everything a session said and did.
_Avoid_: trace, session log, trajectory

### Work

**Item**:
An issue or pull request.
_Avoid_: issuable

**Run**:
Work toward one outcome, on one item or for one workflow.
_Avoid_: job, task, build, execution

**Stage**:
A part of the pipeline that a run does, such as triage.
_Avoid_: phase, step

**Workflow**:
Instructions for an agent, paired with a trigger that says when they run.
_Avoid_: action, automation, routine, job, scheduled task

**Trigger**:
The condition under which a workflow runs.
_Avoid_: event

**Status label**:
One of the four labels that say what an issue needs next: `needs-triage`,
`needs-info`, `ready-for-human`, `ready-for-agent`.
_Avoid_: state label, workflow label

**Claim**:
Linewright's assignment to an item: it has taken the item on.
_Avoid_: lock, lease, take

**In review**:
An issue's status while an open pull request closes it.
_Avoid_: pending, in progress

**Ticket**:
An issue an agent implements.
_Avoid_: task, story

**Tracking issue**:
The parent issue of the tickets cut from one spec change.
_Avoid_: epic

**Usage limit**:
The cap a subscription puts on how much its agents may run in a window, such
as five hours or a week, beyond which the agent runtime refuses sessions until
a stated reset time.
_Avoid_: rate limit, session limit, quota
