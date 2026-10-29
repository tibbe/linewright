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
A person who steers the repository: writes its spec, labels its issues, and
reviews and merges its pull requests.
_Avoid_: owner, reviewer, human

**Reporter**:
A GitHub user who files an issue.
_Avoid_: customer, author

**Agent**:
What works on code on its own in a session: the model, acting through the
harness.
_Avoid_: bot, worker, assistant, model, LLM, the AI, Claude

**Harness**:
The program that runs an agent's sessions: the loop, tools, context and
permission gating around the model, such as Claude Code. The agent harness,
not an evaluation harness.
_Avoid_: agent runtime, runtime, scaffold, engine, CLI, binary, coding agent,
coding tool, framework, platform, assistant

**Session**:
An agent's work in one context, from its first prompt to its last.
_Avoid_: conversation, task

**Transcript**:
The record of everything a session said and did.
_Avoid_: trace, session log, trajectory

**Brief**:
What a session works from: the prompts it is sent, with its task and result
contract, and its skills.
_Avoid_: system prompt, task description, playbook, template

**Prompt**:
A template holding a session's task and result contract, which Linewright
fills in and sends the session.
_Avoid_: instructions, brief

**Attachment**:
A file a session makes for Linewright to post with the Markdown of its result.
_Avoid_: artifact, asset, media

**Setup command**:
The command that prepares a session's clone before the session starts, such
as by installing dependencies.
_Avoid_: setup script, setup steps, setup hook, init script, install script,
bootstrap, provisioning, lifecycle script, post-create command, pre-start
hook, build script

**Session setting**:
A setting that shapes the sessions of a stage or an automation, such as its
model or its setup command.

### Work

**Item**:
An issue or pull request.
_Avoid_: issuable

**Run**:
Work toward one outcome, on one item or for one automation.
_Avoid_: job, task, build, execution

**Stage**:
A part of the pipeline that a run does, such as triage.
_Avoid_: phase, step

**Outcome**:
How a run ended, such as done or handed back, or that it has not ended yet.
_Avoid_: result, status, state, exit

**Automation**:
Instructions for an agent, paired with a trigger that says when they run.
_Avoid_: workflow, action, routine, job, scheduled task

**Trigger**:
The condition under which an automation runs.
_Avoid_: event

**Status label**:
One of the four labels that say what an issue needs next: `needs-triage`,
`needs-info`, `ready-for-human`, `ready-for-agent`.
_Avoid_: state label, workflow label

**Claim**:
Linewright's assignment to an item: it has taken the item on.
_Avoid_: lock, lease, take

**Hand-back**:
Linewright's letting go of an item it cannot finish, with the reason, for a
person to take on.
_Avoid_: handoff, escalation, reassignment

**Stall**:
A hand-back in which Linewright judged nothing: something it relies on
failed it, and a person fixes what failed or has Linewright try again.
_Avoid_: blocked, broken, stuck, failed, errored, error, action required,
needs attention

**In review**:
An issue's status while an open pull request closes it.
_Avoid_: pending, in progress

**Marked for Linewright**:
An item Linewright may start work on of its own accord: every item, or, where
the operator requires a label, those carrying it.
_Avoid_: enabled, trusted

**Design**:
The structure of the code for one item: its parts, what each is responsible
for, and the interfaces between them.
_Avoid_: technical design, detailed design, high-level design, low-level
design, approach, blueprint, design doc, tech spec, RFC

**Ticket**:
An issue an agent implements.
_Avoid_: task, story

**Tracking issue**:
The parent issue of the tickets cut from one item.
_Avoid_: epic

**Lane**:
The board's row for one piece of work: a tracking issue with its tickets, or
an item on its own.

**Card status**:
How pressing what a board card's line reports is.
_Avoid_: state, flag, priority

**Worked**:
Of an item: Linewright has opened it, assigned itself to it, labeled it, or
opened a pull request that closes it.

**Managed**:
Of an item: Linewright has worked it, or it waits on Linewright to act next.
_Avoid_: tracked, watched, monitored, in the pool

**Usage limit**:
The cap a subscription puts on how much its agents may run in a window, such
as five hours or a week, beyond which the harness refuses sessions until
a stated reset time.
_Avoid_: rate limit, session limit, quota

### Operation

**Layer**:
One of the ranked places a setting or a prompt file can be set, such as a
flag or the repository's file, each the repository's, the operator's or
Linewright's own.
_Avoid_: scope, level, tier, source, overlay, override, drop-in

**Drain**:
The ending of `serve` that begins no new work and lets the runs in flight
end.
_Avoid_: graceful shutdown, graceful stop, soft stop, warm shutdown

**Stop**:
The ending of `serve` that cuts the runs in flight short, keeping their
unfinished work.
_Avoid_: forceful stop, fast shutdown, cold shutdown, hard stop
