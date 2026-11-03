# Sessions

An agent session works in a world Linewright builds for it, on a host it
shares with the sessions running beside it. Linewright gives a session what
its work needs, and limits it where that costs its work nothing.

For example, the operator passes `NPM_TOKEN` so that an implement session can
install the project's private packages.

## User Stories

1. As an operator, I want to choose what agent sessions can do on GitHub, so
   that an agent can't change more than I let it.
2. As an operator, I want to choose what of my environment variables sessions
   get, so that they have the project's tools and credentials, and Linewright's
   own only where I let them.
3. As an operator, I want each session to start from a clean world, so that
   its work doesn't depend on what an earlier session or my own setup left
   behind.
4. As an operator, I want sessions running at once kept from each other, so
   that work in parallel doesn't interfere.
5. As a maintainer, I want the repository made ready for agents once, not in
   every session, so that sessions don't spend their time and the host's on
   the same setup.

## What a session has

A session gets the capabilities its work needs, and is limited where that
costs its work nothing.

- **Its capabilities.** A shell, the web and GitHub, which its agent is
  trusted to use for its work.
- **A session's skills and tools.** A session or sub-agent has Linewright's
  skills and the repository's own, and only the tools its work needs.
- **Not prevented.** An agent writing to GitHub outside its result is not
  prevented.
- **The sessions' token.** Optionally, a GitHub token for agent sessions
  (`LINEWRIGHT_SESSION_GITHUB_TOKEN`), which they are given in place of
  Linewright's, so that what a session may do on GitHub, such as only read,
  is the operator's to choose. Without one, sessions are given Linewright's
  token, and Linewright warns at startup that sessions can write to GitHub.
- **The shell.** Agents' commands run in `bash`, whatever shell the operator
  uses.
- **As installed.** A session never updates the harness, in the background
  or on an agent's command: every session runs the harness the operator
  installed.
- **No person takes part in a session.** A question for a person comes back
  as the session's outcome.

## Environment variables

A session's environment variables hold only the variables the operator lets
through and those Linewright sets.

- **Passed.** The variables this table or `session-pass-env` names.
- **Set by the operator.** The values `session-env` sets.
- **Set by Linewright.** The session's GitHub token, when it has one, as
  `GH_TOKEN` and `GITHUB_TOKEN`, its own temporary directory as `TMPDIR`, and
  the harness's settings that the rules under
  [What a session has](#what-a-session-has) need, such as the shell.

| variables                                                                | what for                                           |
| ------------------------------------------------------------------------ | -------------------------------------------------- |
| `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`                               | finding programs and the user's home               |
| `LANG`, `LANGUAGE`, `LC_*`, `TZ`                                         | locale and time zone                               |
| `TERM`                                                                   | the terminal tools format for                      |
| `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`   | where tools keep their configuration and caches    |
| `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY`, `ALL_PROXY`, in either case     | reaching the network through the host's proxy      |
| `SSL_CERT_FILE`, `SSL_CERT_DIR`, `NODE_EXTRA_CA_CERTS`                   | trusting the host's certificate authorities        |
| `ANTHROPIC_*`, `CLAUDE_*`                                                | the harness's configuration and credentials        |

- **Patterns.** A name ending in `*` names every variable with that prefix.
- **The harness's namespaces.** Of the harness's variables, the table passes
  those in the namespaces it owns; a setting outside them, such as
  `DISABLE_TELEMETRY`, the operator passes or sets like any other variable.
- **Which value wins.** A variable Linewright sets itself replaces one of the
  same name the table or `session-pass-env` passes, and a value `session-env`
  sets replaces a passed one.
- **Linewright's own.** `session-env` setting a variable Linewright may set
  itself is an error at startup.
- **Set values.** `session-env`'s values are strings, taken as written, such
  as a `$`, which refers to no variable, or an empty string, which sets the
  variable empty.
- **What environment variables can't hold.** A name or value `session-env` sets
  that environment variables can't hold is an error at startup, such as an empty
  name, an `=` in a name or a NUL byte in either.
- **Credentials passed.** The credentials the table passes are those its
  variables hold, where each variable's format puts them:

  | variables                                    | the credentials                                                                                                  |
  | -------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
  | those the harness reads a credential from    | the value                                                                                                        |
  | the proxy variables                          | the password in the URL, both as written and percent-decoded                                                     |
  | those the harness reads request headers from | each header's value, both whole and, when it opens with a scheme, without it, as in `Authorization: Bearer …` |
- **Never passed.** `session-pass-env` never passes Linewright's GitHub
  tokens or the webhook secret: a pattern skips them, and naming one is an
  error at startup.
- **Logged at startup.** At startup Linewright logs the names, never the
  values, of the variables in its environment that sessions are not given,
  and, for the whole pipeline and for each stage, the names `session-env`
  sets, a stage's own apart from those it takes from the whole pipeline's.

## Environment

With a setup command, every session's clone starts from the environment: the
repository made ready for agents' work once, by the setup command, rather than
in each session.

- **The setup command.** The `setup` table's `command`
  ([Configuration](operation.md#configuration)), run by `bash` in the root of
  a clone of the default branch at its head, with nothing on its standard
  input, with the whole pipeline's environment variables
  ([Environment variables](#environment-variables)) and a temporary directory
  of its own as `TMPDIR`.
- **No setup command.** Without one, a session's clone is a plain clone of
  its branch. An environment built earlier is kept, unused, until a setup
  command is set again.
- **A build.** Running the setup command is a build of the environment. A
  build succeeds when the command exits with status 0 within the `setup`
  table's `timeout`, and fails otherwise. A build is not a run. Processes it
  leaves running end when it ends; only its files are kept.
- **What a session's clone gets.** Every file git ignores in the
  environment, copied into the clone, as a copy-on-write copy where the
  filesystem allows one; every tracked file comes from the session's branch.
  A copy that fails is an infrastructure failure of the session
  ([Ending](runs.md#ending)).

### Building

The environment is rebuilt when what it was built from changes, when it grows
old, or on request, and a build replaces it only by succeeding.

- **Triggers.** A build starts on any of these:
  - the setup command, or a value `session-env` or `session-pass-env` gives
    the variables it runs with, changing from what the environment was built
    with;
  - the environment growing older than the `setup` table's `max-age`;
  - `lw environment rebuild`.

| State      | Event                        | Next       | Effect                                                                                       |
| ---------- | ---------------------------- | ---------- | -------------------------------------------------------------------------------------------- |
| none       | `serve` starts, or a trigger | building   | the setup command starts; runs wait as under [Holds](runs.md#holds)                          |
| building   | the build succeeds           | ready      | the build becomes the environment                                                            |
| building   | the build fails              | none       | nothing more                                                                                 |
| ready      | a trigger                    | rebuilding | the setup command starts; sessions go on starting from the environment                       |
| rebuilding | the build succeeds           | ready      | the build replaces the environment                                                           |
| rebuilding | the build fails              | ready      | the environment as last built stays                                                          |
| building   | a trigger                    | building   | another build starts once this one ends                                                      |
| rebuilding | a trigger                    | rebuilding | another build starts once this one ends                                                      |
| building   | `serve` exits                | none       | the build is abandoned                                                                       |
| rebuilding | `serve` exits                | ready      | the build is abandoned                                                                       |

- **A replaced environment.** A session that has already copied the
  environment keeps its copy, and the replaced environment is removed once no
  copy from it is in progress.
- **Kept.** The environment, the commit and time it was built from and the
  output of the latest build and of the latest that failed are kept across
  restarts, with secrets redacted ([Secrets](operation.md#secrets)).
- **Stopping.** A drain waits for no build and starts none. A trigger that
  came while `serve` was down builds once it starts.
- **Without `serve`.** `once` and `automation run` use the environment when
  no trigger applies to it, and otherwise build it first and wait for the
  build. A build that fails while there is an environment is no error of
  theirs. When it fails and there is none, `automation run` runs nothing, and
  `once` does its cycle under the hold as `serve` would
  ([Holds](runs.md#holds)); each then exits with status 1, naming the failed
  build and `lw environment` for its output.
- **`lw environment rebuild`.** It asks the `serve` of the configured `repo`
  on the same host to build, and returns once the build ends; `--no-wait`
  returns once `serve` has taken the request. Without `serve`, it builds the
  environment itself. A request while no build may start, as under a drain
  or a `config.toml` Linewright cannot use
  ([Linewright's files](repository.md#linewrights-files)), is refused, saying
  why, and is not kept for later. Its output and exit statuses are the
  monitoring commands' ([Monitoring](monitoring.md)): it succeeds when what
  it asked for was done, a build that succeeded or, with `--no-wait`, a
  request `serve` took, and fails otherwise, such as when the build failed or
  there is no setup command.

## Isolation

Linewright keeps sessions running at once from disturbing each other, and a
session from depending on what an earlier session left behind or on the
host's user-level setup. Isolation is best effort, of what sessions can
observe of each other, not of where they run: sessions share the host and its
user, so what a session reaches outside its clone is not isolated.

When a turn ends, however it ends, every process it started ends with it,
including those that left its process group or session.

## Out of scope

- One container per run, instead of agents as processes beside Linewright.
- Carrying into a session's clone what the setup command wrote outside its
  own clone.
- Variables only the setup command is given, such as a registry token that
  sessions are not.
- Keeping sessions from, or redacting, credentials stored where Linewright's
  user can read them, such as `~/.config/gh/hosts.yml`, `~/.aws/credentials` or
  the harness's own login. A container, or a user of its own that holds no other
  credentials, is the isolation.
