# Runs

A run is one piece of Linewright's work: one stage on one issue or pull
request, or one workflow. Whatever the stage, runs share how they are
limited, how they end and what happens when they fail.

For example, a fix-ci run on #43 has its push refused because Linewright's
token may not change workflow files. A retry can't change that, so the run
hands #43 back at once, saying which permission to grant.

## User Stories

1. As an operator, I want several items worked at once up to a ceiling I set,
   so that throughput is mine to choose.
2. As an operator, I want a run that cannot finish to hand its item back with
   the reason, so that nothing stalls silently.
3. As an operator, I want a failure retried only when a retry could help, so
   that no session is spent on a run that must fail again.
4. As an operator, I want Linewright to wait out the usage limit and carry on
   where it stopped, so that hitting the limit costs no work and needs no
   person.
5. As an operator, I want each run's outcome and transcript kept, so that I
   can see what Linewright did and why.

## Decisions

### Stages

- A run does one stage on one item, or runs one workflow; a workflow's run has
  no item. The stages:

  | stage | what its run does |
  |---|---|
  | `triage` | triages an issue ([Triage](triage.md)) |
  | `tickets` | cuts a spec change into tickets ([Implementation](implementation.md)) |
  | `implement` | builds a ticket into a pull request ([Implementation](implementation.md)) |
  | `fix-ci` | fixes the CI of Linewright's pull request ([Pull requests](pull_requests.md)) |
  | `rework` | addresses feedback on Linewright's pull request ([Pull requests](pull_requests.md)) |
  | `update` | brings Linewright's pull request up to date ([Pull requests](pull_requests.md)) |

- A run works in agent sessions, each fresh and in its own clone, or in none
  where its work needs no agent, such as an update without a conflict.

### Limits

- Concurrency is a ceiling on simultaneous runs, workflow runs included
  (`max-runs`, default 4).
- Each agent session has a timeout on the time it runs (`session-timeout`,
  default 30 min), unless its workflow sets its own.

### Ending

- A run ends in one of these outcomes, each with its name in JSON:
  - **done** (`done`): it did its work, or found none left to do. A triage
    run is done whatever it decides.
  - **handed back** (`handed-back`): it could not finish and handed its item
    back.
  - **cancelled** (`cancelled`): its item stopped asking for the work, or
    `serve` was stopped.
  - **ended for the usage limit** (`usage-limit`): its session was refused
    for the usage limit under `once` or `workflow run`.
  - **failed** (`failed`): it could not finish, and nothing on GitHub says
    so, such as a workflow's run.
  - **in flight** (`in-flight`): it has not ended.
  - **interrupted** (`interrupted`): the process running it stopped before it
    ended, such as by a crash or a kill.
- Handing an item back is labeling it `ready-for-human` with a comment that
  says why.
- A session's result the manager cannot parse goes back to the session with
  the parse error, so it can answer again (`result-retries`, default once).
- Any unexpected end (a crash, a timeout, the agent giving up, a result that
  still does not parse) hands the run's item back with the reason and the
  tail of the log.
- A failure is retried once first only when a retry could end differently,
  as an infrastructure failure could. One a retry cannot change is handed
  back at once, and the comment says what the maintainer can do about it:
  for a push GitHub refuses because Linewright's token lacks a permission,
  which permission to grant.
- A run is cancelled when its item stops asking for its work, as each stage
  says, or when `serve` stops. A cancelled run posts nothing more, and one
  whose pull request was merged or closed pushes nothing either.
- A restart leaves the runs in flight interrupted.
- Runs are kept across restarts, with their outcomes and their sessions'
  transcripts.

### Usage limit

- When the agent runtime refuses a session for the usage limit and says when
  the limit resets, Linewright starts no session until then, and claims no
  tickets.
- A session refused for the usage limit has not failed. It waits in its
  clone and, once the limit resets, resumes as the same session with the
  time it had left, however long the wait.
- A reset time that has already passed is waited out for a short time.
- A refusal that does not say when the limit resets is an infrastructure
  failure.
- Only `serve` waits. Under `once` and `workflow run`, the refused run ends
  for the usage limit.
- A restart forgets the limit.

## Out of scope

- Resuming, after a restart, a session that was waiting out the usage limit.
- Slowing down as the usage limit approaches, before a session is refused.
