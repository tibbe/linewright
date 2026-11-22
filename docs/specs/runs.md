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
7. As an operator, I want to know what each session of a run cost, so that I
   see where the time and the tokens go.
8. As an operator, I want to follow what an agent is doing as it does it, so
   that I can tell an agent working hard from one that is stuck.
9. As an operator, I want to see what Linewright did on an item before, with
   each run's outcome and transcript, so that I understand how it got where it
   is.
10. As a maintainer, I want to read agents' transcripts, so that I can tune
    the repository's skills and instructions.

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

- **Sessions.** A run works in agent sessions ([Sessions](sessions.md)), each
  fresh and in its own clone, or in none where its work needs no agent, such as
  an update without a conflict.
- **A clone's end.** A clone is removed once its run is done with it, even
  one made only partway.
- **Clones left behind.** As it starts, Linewright removes, alongside its
  work, every clone that no running Linewright is using, such as one left by
  an exit at once ([Stopping](operation.md#stopping)). It logs the clones as
  it begins and how long it took as it ends.

A session starts as its clone starts being made, and goes on along these
steps:

1. Linewright makes the session's clone
   ([Environment](sessions.md#environment)).
2. The maintenance command runs in it ([Maintenance](#maintenance)).
3. The agent is sent its first prompt.

- **1a. The clone cannot be made.** The session ends, as an infrastructure
  failure ([Ending](#ending)).
- **2a. The maintenance command does not exit with status 0.** The agent is
  sent its first prompt all the same.

## Maintenance

A session's maintenance command brings its clone up to date with its branch
and ready for the agent.

- **The command.** The `maintenance` setting
  ([Configuration](operation.md#configuration)), run by `bash` in the root of
  the clone, with nothing on its standard input, with the session's
  environment variables
  ([Environment variables](sessions.md#environment-variables)). With an empty
  one, nothing runs.
- **When.** On the branch as the clone checked it out, before any update of
  the branch with the default branch
  ([Keeping up to date](pull_requests.md#keeping-up-to-date)).
- **What it leaves running.** Processes the maintenance command leaves running
  end once the run is done with the clone.
- **Its output** is kept with the session's transcript
  ([Transcripts](#transcripts)).

## Performance

Linewright copies the environment and removes clones within a margin of the
fastest known way.

- **The fastest known way.** The fastest way that works on the host's
  filesystem, of those its platform documents or other software uses.
- **The margin.** Making a copy of the environment
  ([Environment](sessions.md#environment)), or removing a clone, takes at most
  1.2 times as long as the fastest known way to do it.

## Contract

Each stage's area decides its session's contract: the value Linewright fills
into each of its prompt's placeholders, and the result the session ends with,
which Linewright parses. What the agent does between the two is the prompt's.
A stage's session is sent the prompt [Stages](#stages) names, and an
automation's `automation.md` ([Briefs](operation.md#briefs)). Where
Linewright fills a placeholder with its own wording rather than the item's
data, the wording is a part in `briefs/parts/`. The placeholder's row names the
part or, where there is a part for each case, gives examples. What Linewright
fills into a part, such as a key of the result, is data. Linewright fills in a
list as its items, each data or a part, separated by commas. For a placeholder
alone on its line, it fills in a Markdown bullet list, one item per line.

- **Every prompt.** `{{facts}}` is the repository, its default branch and the
  spec glob (`facts.md`), and `{{output}}` is the stage's result contract,
  `briefs/outputs/<stage>.md`, followed by the session's attachments directory
  and the kinds of file Linewright attaches (`attachments.md`,
  [Attachments](#attachments)).
- **The result** is a JSON object in the session's final message: the whole
  message when it is one, else its last fenced code block when that holds one,
  else the text from its first `{` to its last `}`. Linewright checks it against
  its stage's JSON Schema (draft 2020-12) in
  [`briefs/results/`](../../briefs/results/), whose descriptions say what each
  key holds. A result that does not match, or that breaks a rule its schema's
  descriptions state, is sent back to the session ([Ending](#ending)).
- **Follow-ups.** A session sent back to work is sent a follow-up, with these
  placeholders filled.

  | follow-up                         | placeholder        | value                                                                                                                                  |
  | --------------------------------- | ------------------ | -------------------------------------------------------------------------------------------------------------------------------------- |
  | `follow-ups/result-error.md`      | `{{error}}`        | why the result could not be used: a part for each reason, such as `result-no-json.md` for a final message with no JSON object or `attachment-refused.md` for an attachment that GitHub refuses |
  | `follow-ups/loose-ends.md`        | `{{problem}}`      | what is still wrong with the session's work: a part for each problem, such as `fold-unfinished.md` for unfinished commits on its branch or `held-credential.md` for a secret in its commits |
  | `follow-ups/late-feedback.md`     | `{{feedback}}`     | the feedback posted while the session worked ([Rework](pull_requests.md#rework))                                                       |
  | `follow-ups/late-feedback.md`     | `{{conversation}}` | the pull request's comments and its unresolved threads                                                                                 |
  | `follow-ups/usage-limit-reset.md` | none               |                                                                                                                                        |

## Attachments

Markdown that Linewright posts to GitHub from a session's result shows the
attachments it refers to.

- **The directory.** Each session has an attachments directory of its own,
  outside its clone.
- **Kinds.** Linewright attaches images and videos, which it tells by the
  file's extension, whatever its letter case: `.png`, `.jpg`, `.jpeg`, `.gif`,
  `.webp`, `.svg`, `.mp4`, `.mov` and `.webm`.
- **References.** A reference is the destination of a Markdown image or
  link, or an HTML `img`'s `src`. A reference refers to an attachment when it
  is a file URL, with the scheme `file` in any letter case, or an absolute
  path, which starts with one `/`. Linewright leaves every other reference,
  and every mention outside a reference, as written.
- **Reading a reference.** Linewright reads a reference with the parser of
  the [URL Standard](https://url.spec.whatwg.org/#concept-url-parser), with
  `file:///` as the base. The path of the attachment is the URL's path,
  percent-decoded. A Markdown destination is read as CommonMark reads it, so
  a space between its angle brackets is a space in the path.
- **Refused.** The result goes back to its session ([Ending](#ending)) when
  a reference to an attachment fails one of these tests:
  - An HTML `src` is a
    [valid URL string](https://url.spec.whatwg.org/#valid-url-string), such
    as one with no space in it.
  - The URL has no host, no query and no fragment. The parser reads
    `localhost` as no host.
  - The path is a regular file in the directory, not a missing file or a
    symlink, of a kind that Linewright attaches.
  - GitHub takes the file. GitHub can refuse a file, such as for its size.
- **Posting.** Linewright uploads each attachment that the posted Markdown
  refers to as a GitHub attachment. It uploads the attachment once, however
  many references it has. It puts the GitHub URL in place of each file URL.
  A video embedded alone in its paragraph becomes its bare URL; embedded
  anywhere else, it becomes a link to it.
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

## Cost

The cost of a session is how long it ran, the tokens it used for each model,
and what those tokens cost. The cost of a run is the sum of the costs of its
sessions.

- **Time.** The time of a session is the time that the session timeout counts
  ([Limits](#limits)).
- **Tokens.** Linewright counts four kinds of tokens: input, output,
  cache-write and cache-read tokens.
- **Money.** The cost of the tokens is the estimate in US dollars that the
  harness gives. The operator can pay a different amount, for example under a
  subscription.
- **Follow-ups and subagents.** The cost of a session includes its follow-ups
  and its subagents. Linewright counts each request to the model one time.
- **Run totals.** For a run, Linewright adds up the tokens and their cost for
  each model and for all the models. A run without a session used no tokens.
- **Missing figures.** A figure that the harness does not give is absent, not
  zero. For example, the harness gives no figures for a session that crashed,
  and no cost for a model that it cannot price. A sum with an absent part is
  absent, and the other sums stay. A session without figures makes every sum
  of tokens and of money of its run absent, because it can have used any
  model.

## Ending

A run ends in one of these outcomes, each with its name in JSON.

| Outcome                   | In JSON       | Meaning                                                                              | Shows as ([Status](design_system.md#status)) |
| ------------------------- | ------------- | ------------------------------------------------------------------------------------ | --------- |
| done                      | `done`        | it did its work, or found none left to do. A triage run is done whatever it decides. | done      |
| handed back               | `handed-back` | it could not finish and handed its item back.                                        | waiting   |
| cancelled                 | `cancelled`   | its item stopped asking for the work, or `serve` was stopped.                        | cancelled |
| ended for the usage limit | `usage-limit` | its session was refused for the usage limit under a command other than `serve`.      | cancelled |
| failed                    | `failed`      | it could not finish, and nothing on GitHub says so, such as an automation's run.     | failed    |
| in flight                 | `in-flight`   | it has not ended.                                                                    | in flight |
| interrupted               | `interrupted` | the process running it stopped before it ended, such as by a crash or a kill.        | cancelled |

- **Handing back.** Handing an item back is as under
  [Repository](repository.md).
- **Spec gaps.** A spec gap that blocks the run (the spec cannot decide it)
  hands its item back with the question. A gap that does not block it becomes
  a separate `ready-for-human` issue describing the needed spec change, and
  the run continues. A gap already filed for the item and still open is not
  filed again.
- **A result Linewright can't accept.** A session's result Linewright
  cannot use for a reason the session can fix goes back to the session with
  the reason, so it can answer again (`result-retries`), as a result that
  does not match its schema does ([Contract](#contract)). Work holding a
  secret ([Secrets](operation.md#secrets)) is one such reason, whether
  Linewright finds the secret or GitHub refuses the push over it, and its
  reason names the variable, the file and, in a commit, the commit.
- **Unexpected ends.** Any unexpected end (a crash, a timeout, the agent giving
  up, a result still not accepted) hands the run's item back with the reason and
  the tail of the log. Except when the agent gives up, the reason for the end is
  in the words of the first row that fits:

  | The session ends as | The reason |
  | --- | --- |
  | it reaches its `session-timeout` | "it timed out after DURATION", where DURATION is the `session-timeout`, such as "30m" |
  | Linewright cannot accept its last result ([A result Linewright can't accept](#ending)) | "its result was not accepted: WHY", where WHY is what Linewright last sent back to the session |
  | it fails on its infrastructure, and again on the retry ([Retries](#ending)) | "it failed on its infrastructure: ERROR", where ERROR is what the part that failed gives, such as the harness's API |
  | the harness ends it with an error | "the harness ended it with `KIND`", where KIND is the harness's name for the error, such as `error_max_turns` |
  | the harness stops without a result, such as in a crash | "the harness stopped without a result" |
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
  ([Architecture](architecture.md#the-protocol-between-linewright-and-an-agent)),
  or, for a run without a session, from its push on, a run is cancelled when its
  item stops asking for it, as under [Steering](repository.md#steering), or when
  `serve` stops. A cancelled run posts nothing more, and one whose pull request
  was merged or closed pushes nothing either.
- **Restarts.** A restart leaves the runs in flight interrupted.
- **Kept.** Runs are kept across restarts, with their outcomes and their
  sessions' transcripts and attachments directories, and with each session's
  cost ([Cost](#cost)) and its model and effort, as the
  harness reports it ran them or else as they were set, and where each of its
  settings and brief files came from: the repository, the operator or
  Linewright's own.

## Holds

While a hold is in force Linewright starts none of the runs it holds and
claims nothing it would start one on; the items wait, and start once the hold
ends.

| Hold            | In force                                                                      | Holds                       | Ends when                |
| --------------- | ----------------------------------------------------------------------------- | --------------------------- | ------------------------ |
| a drain         | `serve` is draining or stopping ([Stopping](operation.md#stopping))           | every run                   | `serve` restarts         |
| the usage limit | the harness has refused a session for it, saying when it resets ([Usage limit](#usage-limit)) | every run | the usage limit resets   |
| the environment | there is a setup command and no environment ([Building](sessions.md#building)) | every run                  | the environment is built |
| a file          | a file of the repository's cannot be used ([Linewright's files](repository.md#linewrights-files)) | the runs that could read it | the default branch fixes the file |

- **Every run.** A hold holds a run whether or not it would start a session,
  such as an `update` ([Stages](#stages)).
- **Claims.** A drain holds every claim
  ([Assignment](repository.md#assignment)).
- **Rank.** The holds are ranked as the table lists them. Where more than one
  is in force, a line with room for one hold names the first, and a list
  names each in that order.

## Usage limit

When the harness refuses a session for the usage limit and says when the
limit resets, the usage limit holds runs until then ([Holds](#holds)).

- **The refused session.** A session refused for the usage limit has not
  failed. It waits in its clone and, once the limit resets, resumes as the
  same session with the time it had left, however long the wait.
- **Resuming.** The resumed session is sent `follow-ups/usage-limit-reset.md`.
- **A reset time already passed.** It is waited out for a short time.
- **No reset time.** A refusal that does not say when the limit resets is an
  infrastructure failure.
- **Only `serve` waits.** Under any other command, the refused run ends for
  the usage limit.
- **Restarts.** A restart forgets the limit.

## Transcripts

- **Where it shows.** [Run](#run) and [`logs`](#lw-logs) each lay out a
  transcript in their own way, and each can show more than the rules below.
- **Sessions and turns.** A run's transcripts show in the order of its
  sessions. Where the rules below leave a choice, a transcript shows as the
  agent itself shows it. Each turn after the first starts with the follow-up
  that started it.
- **Entries.** The agent's messages show in full. Each tool call shows on one
  line: the tool, its main argument and a short account of its result, such
  as the lines read, the exit status or, for a sub-agent, the count of its
  tool calls and how long it ran. A sub-agent's work shows indented under the
  call that started it.
- **Thinking.** Where the agent thought before its next step, a "thought"
  entry shows how long it thought. The time starts at the last event before
  the thinking, such as the prompt, a tool call that returned or, for a
  sub-agent's first thought, the call that started the sub-agent. The time
  ends when the thinking ends.
- **A turn's length.** Each turn ends with an entry saying "worked for" and
  how long the turn took, from its prompt to its last entry.
- **Cut short.** A turn can end before its "thought" entry ends, such as
  when the session ends or the harness refuses the turn for the usage limit.
  The entry then shows `interrupted` and how long it went on until the turn
  ended. A "worked for" entry whose session ends first shows the same. Where
  Linewright does not know when the turn or session ended, the entry shows
  no time.
- **The clone.** Linewright makes a clone for each session
  ([Stages](#stages)). This is an entry: a line with how long it took, then
  the error if it failed.
- **Maintenance.** The maintenance command is an entry
  ([Maintenance](#maintenance)): a line with the command, its exit status
  and how long it ran, then its output, which shows as a tool call's full
  result does, and always when the command failed. An empty one shows
  nothing.
- **Running.** A tool call, the clone and the maintenance command are entries
  that run. Each shows `running` in place of its account until it ends. An
  entry whose session ends first shows `interrupted`. `running` shows as in
  flight, and `interrupted` as cancelled ([Status](design_system.md#status)).
- **In full.** A transcript in full shows everything it holds, including the
  agent's thinking, each tool's full input and result, and the brief and
  follow-ups.
- **Followed.** A followed transcript shows each entry as it happens. An entry
  that runs shows once it starts, and again with its account, at the time,
  once it ends: a tool call when it returns or ends in the background.

## `lw runs`

`runs` lists every run, newest first, including runs that started no session:
its run ID, item, stage, start, duration and outcome.

- **One item.** `runs N` lists #N's, each with its sessions' model and
  effort; `--json` also gives where each setting and brief file came from,
  as the record holds it ([Runs](#ending)).
- **Narrowing.** `--stage`, `--since` and `--limit` narrow the list.
  `--since` keeps the runs that started at or after a time, or a duration
  ago.
- **Outcome.** A run's outcome is one of those under [Ending](#ending), by its
  name, and in JSON by its name in JSON.
- **Duration.** A run's duration is how long it ran, or for a run in flight
  how long it has run so far. An interrupted run has none: in JSON,
  `duration_seconds` is null.
- **Run ID.** A run ID is `<item>/<stage>-<n>`, the `n`th run of that stage
  on that item, such as `43/rework-2`, or `<automation>/<n>` for an automation's
  runs, with the stages under [Runs](runs.md). It is opaque.

## `lw logs`

`logs N` shows the transcript of #N's run in flight, or else of its latest run
that started a session.

- **Another run.** `--run ID` picks another run.
- **The transcripts.** `logs` shows the run's transcripts as
  [Transcripts](#transcripts) says, as far as text that only
  ever grows allows. `logs --json` gives what the agent gives about them,
  following the agent's own JSON output, and so leaves out the "thought" and
  "worked for" entries, which are Linewright's.
- **Times.** Each entry starts with the time since its session started, as
  h:mm:ss with the hours left out under an hour, such as `2:10` or
  `1:02:10`, not as a duration is shown, so that the entries align.
- **`--verbose`.** It shows the transcripts in full.
- **`--follow`.** It shows the transcripts so far, then follows them, and ends
  when the run ends.
- **Following in JSON.** `logs --json --follow` gives an entry that runs as
  two objects, at the times [Followed](#transcripts) says:
  one as it starts, with what it holds then, and one as it ends, with the
  whole entry.
- **Running in JSON.** Read without following, `logs --json` gives an entry
  still running as one object with what it holds so far and null for the
  rest.
- **The clone.** In `logs --json`, the clone's object holds how long it took
  and, when it failed, the error.
- **Maintenance.** In `logs --json`, the maintenance command's object holds
  the command, its exit status, how long it ran and its output, even where
  the transcript as text wouldn't show the output.
- **No session.** For a run that started no session, `logs` says so.

## `lw export`

`export ID DIR` writes run ID into DIR in the form that the output of a
trial gives its run. That is the outcome of the run and the record of each
session ([The output of a trial](benchmarks.md#the-output-of-a-trial)).

- **DIR.** Linewright makes and refuses DIR as it makes and refuses the
  directory of the output of a trial.
- **A run in flight.** Linewright writes the run as it is when the command
  starts.

## Screens

| Screen | Reached from                                                                                                                         | Purpose                                                                    |
| ------ | ------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------- |
| Runs   | the navigation ([Shell](shell.md)), and the last row of Item's runs ([Item](board.md#item))                               | find a run                                                                 |
| Run    | a run in a list of runs, on Automations or on Benchmark, and the notice of an automation whose last run failed                          | read a run's transcripts, follow a run in flight, and capture it as a task |

### Runs

- **Layout.** The list of
  runs, with its filters.
- **Filters.** Filters for the item, the stage and the time since which runs
  started narrow the list.
- **The list of runs.** A table with a row per run, newest first.

| Column   | Holds                                                                                          |
| -------- | ---------------------------------------------------------------------------------------------- |
| Outcome  | the run's outcome                                                                              |
| Run      | the run ID                                                                                     |
| Item     | the item; for an automation's run, "automation" and its name, linking to Automations           |
| Stage    | the run's stage                                                                                |
| Started  | when the run started, with an automation's run by hand marked there                            |
| Duration | the run's duration                                                                             |

### Run

- **Layout.** The heading at
  the top, then the run's transcripts ([Transcripts](#transcripts)), each under
  its session's heading.
- **Heading.** The run ID, then ", by hand" for an automation's run by hand,
  and "(it started no session)" for a run that ended without a session. Under
  them is the run's item, as in the Item column of [Runs](#runs-1).
- **A session's heading.** The session's number and when it started, such as
  "session 1: started 5 minutes ago", then how long it has run. This time does
  not include a wait for the usage limit to reset. Where Linewright does not
  know when the session ended, such as when a restart interrupted its run,
  the time goes up to its last entry.
- **A session's settings.** Under its heading, the session's model and effort
  open on click to where its settings and brief files came from.
- **Views.** The screen shows the transcripts as the agent's own default view
  does. With the verbose switch, it shows them as the agent's verbose view
  does: for example, the agent's thinking is open only with the switch. The
  screen never shows what the agent was told other than its prompts.
- **Prompts.** The brief and each follow-up are entries, each shown as its
  first line.
- **Durations.** Each entry that runs shows how long it ran, and the time
  goes up while the entry runs. If its session ends first, the time stops
  there, and where Linewright does not know that end, the entry shows no time.
  The entry's account does not repeat the time.
- **Working.** While a followed session's turn goes on and no entry runs, the
  session's last line says "working" and how long the turn has gone on. The time
  goes up as the turn goes on. Between turns, there is no such line. While the
  session waits for the usage limit to reset, the line says so and until when
  instead, such as "waiting for the usage limit, until 14:30".
- **Folding.** An entry opens on click to what its line leaves out: a tool
  call's full input and result and its sub-agent's work, the clone's error, the
  maintenance command's output, the full text of the brief or a follow-up, or
  the thinking a "thought" entry stands for. A "thought" entry with no thinking
  text does not open. An entry that failed starts open. A sub-agent's work
  starts folded to the line of the call that started it, with the count of its
  entries.
- **Verbose.** The verbose switch is set once for the screen, and opens
  everything, a session's settings too.
- **Following.** A run in flight is followed. The screen stays at the bottom
  until the reader scrolls up, and then offers "Jump to the latest entry".
- **Capturing.** A run that can be captured
  ([Capturing](benchmarks.md#capturing)) has "Add task" with the screen's
  buttons ([Shell](shell.md)). It opens a dialog titled "Add task to a
  benchmark" ([Making](design_system.md#making)), with these fields, then "Add
  task":

  | Field          | Holds                                                                                                   |
  | -------------- | ------------------------------------------------------------------------------------------------------- |
  | Benchmark      | a choice of the benchmarks of the run's stage, then "New benchmark"                                     |
  | Benchmark name | the name of the new benchmark, shown while "New benchmark" is chosen                                    |
  | Name           | the task's name ([Names and IDs](benchmarks.md#names-and-ids))                                          |
  | Criteria       | the criteria                                                                                            |

  - **A new benchmark's refusals.** Linewright refuses a new benchmark as on
    [New benchmark](benchmarks.md#new-benchmark).
  - **What was made.** The message is "Captured run ID as the task NAME in the
    benchmark BENCHMARK." For a new benchmark, the message is "Captured run ID
    as the task NAME in the new benchmark BENCHMARK." BENCHMARK links to the
    benchmark's Tasks.

## Out of scope

- Resuming, after a restart, a session that was waiting out the usage limit.
- Slowing down as the usage limit approaches, before a session is refused.
- Telling the agent that its session's maintenance command failed.
- A teardown command, run as a clone is removed to release what lives outside
  it, such as a container its maintenance command started.
- A bound on how long a plain clone of a branch takes, which the network sets.
- Removing old runs, with their transcripts and attachments.
- Showing what a run cost, on Runs and Run or in the output of `runs` and
  `logs`.
- A link of its own to each transcript entry.
- Pricing tokens at prices other than the harness's own.
