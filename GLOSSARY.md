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
The program that runs the agent in a session: the loop, tools, context and
permission gating around the model, such as Claude Code. The agent harness,
not an evaluation harness.
_Avoid_: agent runtime, runtime, scaffold, engine, CLI, binary, coding agent,
coding tool, framework, platform, assistant

**Session**:
Linewright's work in one clone for a run, by an agent in one context.
_Avoid_: conversation, task

**Transcript**:
The record of everything a session said and did.
_Avoid_: trace, session log, trajectory

**Turn**:
A prompt sent to a session and the agent's work on it until it stops.
_Avoid_: interaction, run

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

**Environment**:
The repository made ready for agents' work, such as with its dependencies
installed, that sessions' clones start from.
_Avoid_: snapshot, prebuild, prebuilt workspace, cached container, cache,
template

**Setup command**:
The command that makes the environment ready, such as by installing
dependencies.
_Avoid_: setup script, install command, initialize command, init task,
on-create command, update-content command

**Maintenance command**:
The command that makes a session's clone ready for its agent, such as by
starting services.
_Avoid_: maintenance script, setup script, setup steps, start command,
session-start hook, post-create command, post-start command, pre-start hook

**Session setting**:
A setting that shapes the sessions of a stage or an automation, such as its
model or its timeout.

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

**Case**:
A run of a stage, captured so that the stage can be run again on what the run
was given.
_Avoid_: task, fixture, sample, example, scenario

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
A hand-back forced on Linewright by a failure of something it relies on,
rather than by its own judgement.
_Avoid_: blocked, broken, stuck, failed, errored, error, action required,
needs attention

**Feedback**:
A comment or review on a pull request Linewright opened, from anyone else,
that a rework run is to address.
_Avoid_: review comments, change requests

**Merge method**:
How a repository's pull requests land on its default branch.
_Avoid_: merge strategy, merge style

**Round**:
Of a pull request Linewright works: the stretch of its work over which
Linewright counts its fix attempts and failed merges.
_Avoid_: cycle, attempt

**State**:
Of an item: where it stands in the pipeline, such as `needs-info`, in review
or tracking.
_Avoid_: status, stage, phase, column

**In review**:
An issue's state while its work waits on the review of a pull request that
closes it.
_Avoid_: pending, in progress

**Done**:
An issue's state once its work has ended.
_Avoid_: closed, resolved, completed

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

**Party**:
A person with a role in an item: who opened it, who is assigned to it, or who
reviews it.

**Worked**:
Of an item: Linewright has done work on it that GitHub shows as its own, such
as opening, claiming or labeling it.

**Managed**:
Of an item: one Linewright counts as its work, whether to come, under way or
done.
_Avoid_: tracked, watched, monitored, in the pool

**Usage limit**:
The cap a subscription puts on how much its agents may run in a window, such
as five hours or a week, beyond which the harness refuses sessions until
a stated reset time.
_Avoid_: rate limit, session limit, quota

**Hold**:
A condition under which Linewright starts no run for a while.
_Avoid_: quiet mode, freeze, gate

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
