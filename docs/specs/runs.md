# Runs

A run is one piece of Linewright's work: one stage on one issue or pull
request, or one automation. Whatever the stage, runs share how they are
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
6. As a maintainer, I want an agent's comments and pull requests to show the
   screenshots and videos it made, so that I see what a change does without
   running it.

## Stages

A run does one stage on one item, or runs one automation; an automation's run
has no item.

| stage       | what its run does                                                                     | prompt         |
| ----------- | ------------------------------------------------------------------------------------- | -------------- |
| `triage`    | triages an issue ([Triage](triage.md))                                                | `triage.md`    |
| `design`    | designs an item, and cuts it into tickets where it needs cutting ([Design](design.md)) | `design.md`    |
| `implement` | builds a ticket into a pull request ([Implementation](implementation.md))             | `implement.md` |
| `fix-ci`    | fixes the CI of Linewright's pull request ([Pull requests](pull_requests.md))         | `fix-ci.md`    |
| `rework`    | addresses feedback on Linewright's pull request ([Pull requests](pull_requests.md))   | `rework.md`    |
| `update`    | brings Linewright's pull request up to date ([Pull requests](pull_requests.md))       | `update.md`    |

- **Sessions.** A run works in agent sessions, each fresh and in its own
  clone, or in none where its work needs no agent, such as an update without
  a conflict.

## Setup

A session starts with its setup command, which prepares the session's clone
before the agent's first prompt.

- **The command.** The `setup` setting
  ([Stage settings](operation.md#stage-settings)), run by `bash` in the root of
  the clone, with nothing on its standard input, in the session's environment
  ([Sessions' environment](operation.md#sessions-environment)). With an empty
  one, the session starts with the agent's first prompt.
- **When.** On the branch as the clone checked it out, before the run brings
  the branch up to date with the default branch
  ([Keeping up to date](pull_requests.md#keeping-up-to-date)).
- **Failing.** A setup command that exits with a non-zero status ends its
  session, as an infrastructure failure
  ([Ending](#ending)).
- **What it leaves running.** Processes the setup command leaves running end
  once the run is done with the clone.
- **Its output** is kept with the session's transcript
  ([Transcripts](design_system.md#transcripts)).

## Contract

Each stage's area decides its session's contract: the value Linewright fills
into each of its prompt's placeholders, and the result the session ends with,
which Linewright parses. What the agent does between the two is the prompt's.
A stage's session is sent the prompt [Stages](#stages) names, and an
automation's `automation.md` ([Prompts](operation.md#prompts)).

- **Every prompt.** `{{facts}}` is the repository, its default branch and the
  spec glob, and `{{output}}` is Linewright's wording of the stage's result
  contract, with the session's attachments directory
  ([Attachments](#attachments)).
- **The result** is a JSON object in the session's final message: the whole
  message when it is one, else its last fenced code block when that holds one,
  else the text from its first `{` to its last `}`. Linewright checks it against
  its stage's JSON Schema (draft 2020-12) in [`results/`](results/), whose
  descriptions say what each key holds. A result that does not match, or that
  breaks a rule its schema's descriptions state, is sent back to the session
  ([Ending](#ending)).
- **Follow-ups.** A session sent back to work is sent a follow-up, with these
  placeholders filled.

  | follow-up                         | placeholder        | value                                                                                                                                  |
  | --------------------------------- | ------------------ | -------------------------------------------------------------------------------------------------------------------------------------- |
  | `follow-ups/result-error.md`      | `{{error}}`        | why the result could not be used                                                                                                       |
  | `follow-ups/loose-ends.md`        | `{{problem}}`      | what is still wrong with the session's work: unfinished commits on its branch, or a secret in its commits, its push or its attachments |
  | `follow-ups/late-feedback.md`     | `{{feedback}}`     | the feedback posted while the session worked ([Rework](pull_requests.md#rework))                                                       |
  | `follow-ups/late-feedback.md`     | `{{conversation}}` | the pull request's comments and its unresolved threads                                                                                 |
  | `follow-ups/usage-limit-reset.md` | none               |                                                                                                                                        |

## Attachments

Markdown that Linewright posts to GitHub from a session's result shows the
attachments it refers to, each uploaded as a GitHub attachment, as a file
dropped into a comment is.

- **The directory.** Each session has an attachments directory of its own,
  outside its clone.
- **References.** The Markdown refers to an attachment by its absolute path
  in the directory, in a reference: the destination of a Markdown image or
  link, or an HTML `img`'s `src`. Any other mention of a path is left as
  written.
- **Refused.** The result goes back to its session ([Ending](#ending)) when
  a reference names a path in the directory that is not a regular file,
  such as a missing file or a symlink, or an absolute path to a file outside
  the directory, or when GitHub refuses an attachment over the file itself,
  such as for its size.
- **Posting.** Linewright uploads each attachment the result's posted
  Markdown refers to, once however many references it has, and puts its URL
  in place of each path. A video embedded alone in its paragraph becomes its
  bare URL; embedded anywhere else, it becomes a link to it.
- **Uploads first.** Applying a result uploads its attachments before any
  other write it makes, the push included.
- **Unused uploads.** An attachment uploaded for a result that is not then
  posted is left unused.

## Limits

Limits bound what runs cost: how many run at once, and how long a session
runs.

- **Concurrency.** A ceiling on simultaneous runs, automation runs included
  (`max-runs`).
- **Session timeout.** Each session has a timeout on the time it runs
  (`session-timeout`). The timeout bounds
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
| ended for the usage limit | `usage-limit` | its session was refused for the usage limit under `once` or `automation run`.      |
| failed                    | `failed`      | it could not finish, and nothing on GitHub says so, such as an automation's run.   |
| in flight                 | `in-flight`   | it has not ended.                                                                  |
| interrupted               | `interrupted` | the process running it stopped before it ended, such as by a crash or a kill.      |

- **Handing back.** Handing an item back is as under
  [Repository](repository.md).
- **Spec gaps.** A spec gap that blocks the run (the spec cannot decide it)
  hands its item back with the question. A gap that does not block it becomes
  a separate `ready-for-human` issue describing the needed spec change, and
  the run continues. A gap already filed for the item and still open is not
  filed again.
- **A result the manager can't accept.** A session's result the manager
  cannot use for a reason the session can fix goes back to the session with
  the reason, so it can answer again (`result-retries`), as a result that
  does not match its schema does ([Contract](#contract)). Work holding a
  secret ([Architecture](architecture.md)) is one such reason, whether
  Linewright finds the secret or GitHub refuses the push over it, and its
  reason names the variable, the file and, in a commit, the commit.
- **Unexpected ends.** Any unexpected end (a crash, a timeout, the agent
  giving up, a result still not accepted) hands the run's item back with the
  reason and the tail of the log.
- **Retries.** A failure is retried once first only when a retry could end
  differently, as an infrastructure failure could. One a retry cannot change
  is handed back at once, and the comment says what the maintainer can do
  about it: for a push GitHub refuses because Linewright's token lacks a
  permission, which permission to grant, and for an attachment GitHub
  refuses over Linewright's token, what the token needs.
- **Unfinished work.** A run that works on a branch and ends before it
  finishes pushes its work so far to the branch, with what its session left
  uncommitted as an unfinished commit. The next run on the branch folds the
  unfinished commit into its own commits. Work that holds a secret is
  not pushed.
- **Cancelling.** Except while it applies a result
  ([Architecture](architecture.md#the-manageragent-protocol)), or, for a run
  without a session, from its push on, a run is cancelled when its item stops
  asking for its work, as under [Repository](repository.md), or when `serve`
  stops. A cancelled run posts nothing more, and one whose pull request was
  merged or closed pushes nothing either.
- **Restarts.** A restart leaves the runs in flight interrupted.
- **Kept.** Runs are kept across restarts, with their outcomes and their
  sessions' transcripts and attachments directories, and with each session's
  model and effort, as the
  harness reports it ran them or else as they were set, and where each of its
  settings and prompt files came from: the repository, the operator or
  Linewright's own, or, for a model or effort none set, the harness.

## Usage limit

When the harness refuses a session for the usage limit and says when
the limit resets, Linewright starts no session until then, and claims no
tickets.

- **The refused session.** A session refused for the usage limit has not
  failed. It waits in its clone and, once the limit resets, resumes as the
  same session with the time it had left, however long the wait.
- **A reset time already passed.** It is waited out for a short time.
- **No reset time.** A refusal that does not say when the limit resets is an
  infrastructure failure.
- **Only `serve` waits.** Under `once` and `automation run`, the refused run
  ends for the usage limit.
- **Restarts.** A restart forgets the limit.

## Out of scope

- Resuming, after a restart, a session that was waiting out the usage limit.
- Slowing down as the usage limit approaches, before a session is refused.
- A teardown command, run as a clone is removed to release what lives outside
  it, such as a container its setup command started.
