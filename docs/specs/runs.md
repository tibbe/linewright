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

## Stages

A run does one stage on one item, or runs one workflow; a workflow's run has
no item.

| stage       | what its run does                                                               |
| ----------- | ------------------------------------------------------------------------------- |
| `triage`    | triages an issue ([Triage](triage.md))                                          |
| `tickets`   | cuts a spec change into tickets ([Implementation](implementation.md))           |
| `implement` | builds a ticket into a pull request ([Implementation](implementation.md))       |
| `fix-ci`    | fixes the CI of Linewright's pull request ([Pull requests](pull_requests.md))   |
| `rework`    | addresses feedback on Linewright's pull request ([Pull requests](pull_requests.md)) |
| `update`    | brings Linewright's pull request up to date ([Pull requests](pull_requests.md)) |

- **Sessions.** A run works in agent sessions, each fresh and in its own
  clone, or in none where its work needs no agent, such as an update without
  a conflict.

## Limits

Limits bound what runs cost: how many run at once, and how long a session
runs.

- **Concurrency.** A ceiling on simultaneous runs, workflow runs included
  (`max-runs`).
- **Session timeout.** Each agent session has a timeout on the time it runs
  (`session-timeout`), unless its workflow sets its own. The timeout bounds
  what a run costs and ends a session that stalls, so a run that times out is
  not retried: it hands its item back, and the comment says the work may need
  cutting smaller or a longer timeout.

## Ending

A run ends in one of these outcomes, each with its name in JSON.

| Outcome                   | In JSON       | Meaning                                                                            |
| ------------------------- | ------------- | ---------------------------------------------------------------------------------- |
| done                      | `done`        | it did its work, or found none left to do. A triage run is done whatever it decides. |
| handed back               | `handed-back` | it could not finish and handed its item back.                                      |
| cancelled                 | `cancelled`   | its item stopped asking for the work, or `serve` was stopped.                      |
| ended for the usage limit | `usage-limit` | its session was refused for the usage limit under `once` or `workflow run`.        |
| failed                    | `failed`      | it could not finish, and nothing on GitHub says so, such as a workflow's run.      |
| in flight                 | `in-flight`   | it has not ended.                                                                  |
| interrupted               | `interrupted` | the process running it stopped before it ended, such as by a crash or a kill.      |

- **Handing back.** Handing an item back is as under
  [Repository](repository.md).
- **A result the manager can't accept.** A session's result the manager
  cannot parse, or whose work holds a credential
  ([Architecture](architecture.md)), whether Linewright finds it or GitHub
  refuses the push over it, goes back to the session with the reason, so it
  can answer again (`result-retries`). The reason names the variable, the
  file and the commit.
- **Unexpected ends.** Any unexpected end (a crash, a timeout, the agent
  giving up, a result still not accepted) hands the run's item back with the
  reason and the tail of the log.
- **Retries.** A failure is retried once first only when a retry could end
  differently, as an infrastructure failure could. One a retry cannot change
  is handed back at once, and the comment says what the maintainer can do
  about it: for a push GitHub refuses because Linewright's token lacks a
  permission, which permission to grant.
- **Unfinished work.** A run that works on a branch and ends before it
  finishes pushes its work so far to the branch, with what its session left
  uncommitted as an unfinished commit. The next run on the branch folds the
  unfinished commit into its own commits. Work that holds a credential is
  not pushed.
- **Cancelling.** A run is cancelled when its item stops asking for its work,
  as under [Repository](repository.md), or when `serve` stops. A cancelled
  run posts nothing more, and one whose pull request was merged or closed
  pushes nothing either.
- **Restarts.** A restart leaves the runs in flight interrupted.
- **Kept.** Runs are kept across restarts, with their outcomes and their
  sessions' transcripts.

## Usage limit

When the agent runtime refuses a session for the usage limit and says when
the limit resets, Linewright starts no session until then, and claims no
tickets.

- **The refused session.** A session refused for the usage limit has not
  failed. It waits in its clone and, once the limit resets, resumes as the
  same session with the time it had left, however long the wait.
- **A reset time already passed.** It is waited out for a short time.
- **No reset time.** A refusal that does not say when the limit resets is an
  infrastructure failure.
- **Only `serve` waits.** Under `once` and `workflow run`, the refused run
  ends for the usage limit.
- **Restarts.** A restart forgets the limit.

## Out of scope

- Resuming, after a restart, a session that was waiting out the usage limit.
- Slowing down as the usage limit approaches, before a session is refused.
