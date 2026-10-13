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
Someone who files an issue, such as a customer.
_Avoid_: customer, author

**Agent**:
A coding-agent session Linewright runs.
_Avoid_: bot, model, worker

**Machine user**:
The GitHub user account Linewright acts as.
_Avoid_: bot account, service account

### Work

**Workflow**:
Instructions for an agent, paired with a trigger that says when they run.
_Avoid_: action, automation, routine, job, scheduled task

**Trigger**:
The condition under which a workflow runs.
_Avoid_: event

**Spec change**:
A merged pull request that changes the spec.
_Avoid_: spec update

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
