# Access

Linewright shows a team its work, in a browser and to the team's agents. The
team wants it to reach only their own people, each only as far as they are
trusted. Access is how Linewright knows who is asking and what they may do.
A person has the same access in a browser and through an agent. So a
person's agent can do for them what they can do themselves.

For example, a maintainer asks their coding agent why #42 has not moved. The
agent connects to Linewright, and the maintainer signs in once to let it act
as them. The agent then reads #42 as the maintainer would see it in the
browser, and answers that it waits for its checks to pass.

## User Stories

1. As a maintainer, I want to sign in with Google or GitHub, so that I need
   no password of Linewright's own.
2. As a maintainer, I want to connect my agent to Linewright over MCP, signed
   in as me, so that it can do what I can do in the UI.
3. As a maintainer, I want to see the agents I allowed and to end one, so
   that an agent I no longer use loses its access.
4. As an operator, I want the people of my domain or GitHub organization to
   sign up on their own, so that I do not add them one by one.
5. As a manager, I want to change people's roles and disable people, so that
   each person can do only what they should.
6. As an operator, I want to name a manager in the `manager` setting, so
   that I can reach a new Linewright.
7. As an operator, I want to name another manager in the `manager` setting,
   so that I regain access when no manager can sign in.
8. As an operator, I want the log to say who did each action, so that I can
   find out who changed what.

## Roles

Each person has one role. A role gives one access level, and each access
level includes the ones above it in the table.

| Role        | Access level | What the level allows                                                     |
| ----------- | ------------ | ------------------------------------------------------------------------- |
| viewer      | read         | to see what the UI shows, and to end one's own authorizations             |
| contributor | act          | to act on the work, such as to capture a run or to launch a benchmark run |
| manager     | manage       | to change people's roles, and to disable and enable people                |

- **At each request.** Linewright checks each request against the role that
  the person has at that time.

## Actions

Each thing that a person can do in the UI is one action, and each action
needs one access level. The UI and the MCP server both give every action,
with the same access level. The exceptions are what a person does to sign in
and out and to allow a client: only the UI gives them.

| Action                              | Access level | In the UI                                                                | Over MCP               |
| ----------------------------------- | ------------ | ------------------------------------------------------------------------ | ---------------------- |
| see the board                       | read         | [Board](board.md#board-1)                                                | `get_board`            |
| see an item                         | read         | [Item](board.md#item)                                                    | `get_item`             |
| see the runs                        | read         | [Runs](runs.md#runs-1)                                                   | `list_runs`            |
| see a run                           | read         | [Run](runs.md#run)                                                       | `get_run`              |
| see the automations                 | read         | [Automations](automations.md#automations-1)                              | `list_automations`     |
| see the benchmarks                  | read         | [Benchmarks](benchmarks.md#benchmarks-2)                                 | `list_benchmarks`      |
| see a benchmark                     | read         | [Benchmark](benchmarks.md#benchmark)                                     | `get_benchmark`        |
| see a task                          | read         | [Task](benchmarks.md#task)                                               | `get_task`             |
| see a benchmark run                 | read         | [Benchmark run](benchmarks.md#benchmark-run)                             | `get_benchmark_run`    |
| see Linewright itself               | read         | [Server](operation.md#server)                                            | `get_server`           |
| see the people                      | read         | [People](#people-1)                                                      | `list_people`          |
| see one's own authorizations        | read         | [Authorizations](#authorizations)                                        | `list_authorizations`  |
| end one of one's own authorizations | read         | [End authorization](#end-authorization)                                  | `end_authorization`    |
| capture a run as a task             | act          | "Add task" on [Run](runs.md#run), and [New task](benchmarks.md#new-task) | `add_task`             |
| make a benchmark                    | act          | [New benchmark](benchmarks.md#new-benchmark)                             | `create_benchmark`     |
| change a benchmark                  | act          | [Edit benchmark](benchmarks.md#edit-benchmark)                           | `edit_benchmark`       |
| change a task                       | act          | [Edit task](benchmarks.md#edit-task)                                     | `edit_task`            |
| remove a task                       | act          | [Remove task](benchmarks.md#remove-task)                                 | `remove_task`          |
| launch a benchmark run              | act          | [New benchmark run](benchmarks.md#new-benchmark-run)                     | `run_benchmark`        |
| change a configuration              | act          | [Edit configuration](benchmarks.md#edit-configuration)                   | `edit_configuration`   |
| cancel a benchmark run              | act          | [Cancel benchmark run](benchmarks.md#cancel-benchmark-run)               | `cancel_benchmark_run` |
| change a person's role              | manage       | [Edit person](#edit-person)                                              | `edit_person`          |
| disable a person                    | manage       | [Disable person](#disable-person)                                        | `disable_person`       |
| enable a person                     | manage       | "Enable" on [People](#people-1)                                          | `enable_person`        |

- **What the reader's role does not allow.** A screen shows a button only
  when the reader's role allows its action.
- **An access level that is too low.** Linewright refuses the action, with
  "This needs the LEVEL access level." and the level that it needs.
- **Logged.** Linewright's log ([The log](operation.md#the-log)) records each
  action that changes something. It records the person, the action and what
  the action changed. It also records how the action ended: done, refused or
  failed, with the reason.

## People

Linewright keeps its own record of each person who may use the UI and the
MCP server.

- **Who a person is.** A person is their identity: an identity provider and
  the subject that provider gives them, never an email address alone. Each
  person has one identity.
- **How a person is shown.** A person is shown by their name and avatar, as
  the provider gives them at each sign-in, and by their identity
  ([Signing in](#signing-in)). A person without a name is shown by their
  identity in its place. A person without an avatar is shown with the first
  letter of their name in its place.
- **A person's ID.** A number that Linewright gives in the order that it
  adds people, from 1.
- **State.** A person is enabled or disabled.
- **Adding people.** Linewright adds a person only when they first sign in
  ([Signing in](#signing-in)).
- **The named manager.** The identity that the `manager` setting names is a
  manager while the setting names it, and cannot lose the role or be
  disabled. At startup, Linewright makes each person whose identity matched
  the setting at their last sign-in an enabled manager. A person that the
  setting stops naming keeps the role, as any other manager has it.
- **The last manager.** While only one enabled person is a manager, that
  person cannot lose the role or be disabled.
- **Disabling.** Disabling a person signs them out of every browser, and
  ends their authorizations at once
  ([The authorization server](#the-authorization-server)).

## Signing in

A person signs in through an identity provider. Linewright checks each
sign-in against its record of people. The UI and the MCP server use the same
sign-in.

| Identity provider | How Linewright signs a person in | Subject     | Identity shown as  | Sign-up rule                                                                         |
| ----------------- | -------------------------------- | ----------- | ------------------ | ------------------------------------------------------------------------------------ |
| Google            | OpenID Connect                   | its `sub`   | the verified email | the verified email's domain is one of `google-signup-domains`                        |
| GitHub            | OAuth, then GitHub's API         | its user ID | the login          | the person is a member of an organization or a team that `github-signup-orgs` names |

- **The sign-up rule.** An identity provider's sign-up rule is the condition
  that an identity must meet at each sign-in.
- **Settings.** Each identity provider has its client ID and its sign-up
  rule as settings, and its client secret as a credential
  ([Configuration](operation.md#configuration)). Linewright offers only the
  providers that have a client ID.
- **Callbacks.** A provider's callback is `/auth/google/callback` or
  `/auth/github/callback` under the public URL (`public-url`).
- **GitHub's token.** Linewright uses the token that GitHub gives for a
  person only to read them and their memberships, then revokes it. A sign-in
  asks the person for `read:org` while `github-signup-orgs` is set, and for
  no scope otherwise.
- **Matching an identity.** `manager` names an identity as `google:` and an
  email, or `github:` and a login. Linewright compares each ignoring case.
- **Signed in.** After a sign-in, the browser stays signed in for 30 days,
  or until the person signs out.
- **Screens that need no sign-in.** Sign in, Consent and Authorization
  error.
- **Without a sign-in.** A request for any other screen from a browser that
  is not signed in goes to Sign in, and back to the screen after the
  sign-in.
- **An open screen.** A browser that is no longer signed in shows Sign in
  in place of each open screen that needs a sign-in, with the message for
  the reason ([Sign in](#sign-in)).
- **Logged.** Linewright's log records each sign-in, each sign-in that it
  refuses and why, and each sign-out, with the identity.

1. The person chooses an identity provider on [Sign in](#sign-in).
2. The provider signs them in and sends them back to Linewright's callback.
3. Linewright finds the person by their identity.
4. The browser is signed in as that person.

Where more than one of 3a to 3e applies, the first does.

- **2a. The provider refuses or fails.** Sign in says that signing in with
  that provider failed.
- **3a. An email that Google has not verified.** Linewright refuses the
  sign-in.
- **3b. The identity that `manager` names.** Linewright signs it in as a
  manager, whatever the sign-up rule says. Linewright adds it if it is new.
  If its record holds another role or is disabled, Linewright makes it an
  enabled manager.
- **3c. An identity that does not meet the sign-up rule.** Linewright
  refuses the sign-in. If the identity is a person, their record stays.
- **3d. A new identity.** Linewright adds it as a viewer.
- **3e. A disabled person.** Linewright refuses the sign-in.

## The MCP server

`serve` serves the MCP server at `/mcp`, over Streamable HTTP, while it
serves the UI ([Serving the UI](operation.md#serving-the-ui)). It follows
versions 2025-06-18 and 2025-11-25 of the Model Context Protocol. This topic
gives what the protocol leaves to Linewright.

- **No sessions and no stream.** Linewright gives no session ID, and offers
  no stream to a GET.
- **Origin.** Linewright accepts a request that has no `Origin`. The only
  `Origin` that it accepts is the public URL's.
- **Tools.** One tool per action ([Actions](#actions)), with the action as
  its title, in the order of the table of actions. A tool that only shows
  something is marked read-only.
- **Arguments.** A tool takes the ID of the thing that it shows or acts on,
  such as `run_id` or `person_id`. It also takes the filters of its screen
  and the fields of its dialog. Each argument has its name on the screen, in
  snake case.
- **What a tool shows.** A tool that shows something returns what its screen
  shows at the time of the call, as structured content with an output
  schema. A fact that a command's `--json` also gives has the name it has
  there ([Output](operation.md#output)).
- **What a tool changes.** A tool that changes something returns the thing
  that it made, changed, removed or ended.
- **Lists.** A tool that lists returns the first 100 of what it lists, in
  its screen's order, or the first `limit`. A result that leaves some out
  says how many, and gives the `after` value that returns the next ones.
- **Transcripts.** `get_run` returns the latest 100 entries of the run's
  transcripts, or the latest `limit`, and gives them in full with `verbose`.
  A result that leaves some out says how many, and gives the `before` value
  that returns the ones before them.
- **Listed tools.** When a client asks for the list of tools, the list holds
  the tools that the person's role allows.
- **An access level that is too low.** A call to a tool that the person's
  role does not allow gets 403, with the message of the refusal
  ([Actions](#actions)).
- **A wrong argument.** A call with an argument that the tool's input
  schema does not allow returns a tool error with "The argument NAME is not
  valid."
- **A refused call.** A call that Linewright refuses for any other reason
  returns a tool error with the message that the UI shows for the refusal.
- **Too many calls.** A person's clients can call tools 120 times in a
  minute. Each further call in that minute gets 429, with a `Retry-After`
  header.

## The authorization server

Linewright is the authorization server and the resource server of the Model
Context Protocol's authorization rules. The MCP server is the one resource that
it guards. Linewright follows those rules, their security best practices and the
standards that they name. It also follows RFC 9207. This topic gives what they
leave to Linewright.

A client is an app that acts as a person over MCP, such as their coding
agent. An authorization is a person's consent for one client to act as them.

- **Issuer.** The public URL (`public-url`) is the issuer.
- **Endpoints.** Under the public URL: `/oauth/authorize` to authorize,
  `/oauth/token` for tokens and `/oauth/register` to register.
- **Clients.** Every client is a public client. A client has a client ID
  metadata document, or registers by dynamic client registration.
  Linewright registers a client that asks for a client secret as a public
  client, without a secret.
- **Fetching a metadata document.** Where the host has a proxy for HTTPS
  ([Environment variables](sessions.md#environment-variables)), Linewright
  fetches a client's metadata document through it. The proxy, and not
  Linewright, then refuses a private or loopback address.
- **Loopback.** Linewright treats a redirect URI to `localhost` as one to a
  loopback IP address.
- **Grants.** The authorization code grant, with only the `S256` method,
  and the refresh token grant.
- **Scopes.** Linewright has no scopes. It ignores a scope that a client
  asks for.
- **When a request ends.** An authorization request ends when its person
  denies the client, when Linewright signs them in, or 10 minutes after the
  client makes it. A browser that comes back to a request that has ended
  gets [Authorization error](#authorization-error).
- **When an authorization begins.** It begins at its sign-in.
- **Tokens.** A token is an opaque string.
- **A code.** A code works for 10 minutes.
- **An access token.** An access token works for one hour.
- **An ended authorization.** When an authorization ends, its code and its
  tokens stop working.
- **A second use.** A second use of a code or of a refresh token ends its
  authorization.
- **An authorization's ID.** A number that Linewright gives in the order
  that authorizations begin, from 1.
- **How long it lasts.** An authorization lasts 30 days from its sign-in,
  unless it ends earlier. Signing out in the browser leaves it in place.
- **Logged.** Linewright's log records each client that registers, each
  consent that a person gives or denies, with the client, and each
  authorization, with the person and the client.

For each authorization request:

1. The person allows the client on [Consent](#consent).
2. The person signs in through an identity provider
   ([Signing in](#signing-in)), whatever the browser is signed in to.

## Screens

| Screen              | Reached from                                                                        | Purpose                                      |
| ------------------- | ----------------------------------------------------------------------------------- | -------------------------------------------- |
| Sign in             | a screen opened or open without a sign-in, "Allow" on Consent, and "Sign out"               | choose an identity provider to sign in with  |
| Consent             | a client's authorization request                                                    | allow or deny the client                     |
| Authorization error | an authorization request that Linewright cannot send back to its client, or that has ended                   | see why Linewright stopped                   |
| People              | the navigation ([Shell](shell.md))                                                  | see the people, and as a manager change them |
| Edit person         | "Edit" on a person, on People                                                       | change the person's role                     |
| Disable person      | "Disable" on a person, on People                                                    | confirm disabling the person                 |
| Authorizations      | "Authorizations" in the account's menu ([Shell](shell.md))                          | see one's own authorizations, and end one    |
| End authorization   | "End" on an authorization, on Authorizations                                        | confirm ending the authorization             |

### Sign in

- **Address.** `/sign-in`.
- **Layout.** A card in the middle of the page. The card holds the heading,
  then the message, where there is one, then the identity providers.
- **The page** has the card in its middle:
  `flex min-h-screen items-center justify-center p-4`.
- **The card** is drawn as a card is ([Cards](ui_patterns.md#cards)), with
  `w-full max-w-100 p-12`.
- **Heading.** "Sign in to Linewright", and under it the repository's name.
  The heading is in `text-3xl` and `font-bold`, centred, with `mb-2` under it.
  The repository's name is in secondary text, centred, with `mb-6` under it.
- **The message.** It says why the person is on the screen, where there is a
  reason. It is a daisyUI `alert`, with `mb-6` under it. It is in the
  `alert-error` style, but for "You signed out." and "Your sign-in ended.",
  which get the plain `alert`.

  | Reason                                          | Message                                      |
  | ----------------------------------------------- | -------------------------------------------- |
  | the person signed out                           | "You signed out."                            |
  | the 30 days ended                               | "Your sign-in ended."                        |
  | an email that Google has not verified           | "Google has not verified EMAIL."             |
  | an identity that does not meet the sign-up rule | "IDENTITY has no access to this Linewright." |
  | a disabled person                               | "IDENTITY is disabled."                      |
  | the provider refused or failed                  | "Signing in with PROVIDER failed."           |

- **Identity providers.** A button for each identity provider that
  Linewright offers ([Signing in](#signing-in)), Google first: "Sign in with
  Google" and "Sign in with GitHub", each with its provider's logo. The
  buttons are one under the other, in `flex flex-col items-center gap-2`.
- **Google's button** is Google's own "Sign in with Google" button for the
  web, as Google gives it: its light one in the light theme and its dark one
  in the dark theme, at its own size.
- **GitHub's button** is a daisyUI `btn`, not `btn-sm`, with GitHub's mark
  before its label. The mark is GitHub's black one in the light theme and
  its white one in the dark theme.

### Consent

- **Address.** `/oauth/authorize`.
- **Layout.** A card in the middle of the page. The page, the card and the
  heading look as on Sign in. The card holds the heading, then the facts, then
  the warning, where there is one, then "Allow" and "Deny".
- **Heading.** "Allow NAME to act as you?", with the client's name as the client
  gives it, or "an unnamed client" without one.
- **Facts.** [Facts](components.md#facts) in the screen's card, with `mb-6`
  under them.

  | Fact              | Holds                                 | Shown                                    |
  | ----------------- | ------------------------------------- | ---------------------------------------- |
  | Client            | the host of the client ID             | for a client with a metadata document    |
  | Sends you back to | the host of the client's redirect URI | always                                   |

- **The warning.** For a loopback redirect URI: "This client is an app on
  this computer. Linewright cannot check which app." It is a daisyUI `alert`
  in the `alert-warning` style, with `mb-6` under it.
- **"Allow" and "Deny"** are in `grid grid-cols-2 gap-2`. "Allow" is a
  `btn-primary`, with the icon `check`. "Deny" has the icon `x-mark`.

### Authorization error

- **Address.** `/oauth/authorize`.
- **Layout.** A card in the middle of the page. The page, the card and the
  heading look as on Sign in. The card holds the heading "Linewright cannot
  authorize this client", then the message.
- **The message.** A daisyUI `alert` in the `alert-error` style.

  | Case                                                  | Message                                                 |
  | ----------------------------------------------------- | ------------------------------------------------------- |
  | an unknown client                                     | "Linewright does not know this client."                 |
  | a metadata document that Linewright cannot fetch or use | "Linewright cannot use this client's metadata document." |
  | a redirect URI that is not one of the client's        | "This is not a redirect URI of this client."            |
  | a request that has ended                              | "This authorization request has ended. Start again from the client." |

### People

- **Address.** `/people`.
- **Layout.** The table "People" ([Tables](ui_patterns.md#tables)).
- **The table.** A row per person ([People](#people)), in the order of their
  names by the Unicode Collation Algorithm.

  | Column    | Holds                                                                                 |
  | --------- | ------------------------------------------------------------------------------------- |
  | Person    | their avatar, then their name, then their identity provider's logo and their identity |
  | Role      | their role                                                                            |
  | State     | "enabled" or "disabled"                                                               |
  | Signed in | when they last signed in                                                              |

- **A row's buttons.** "Edit" ([Editing](ui_patterns.md#editing)), then
  "Disable" on an enabled person and "Enable" on a disabled one. "Disable"
  has the icon `lock-closed`, and "Enable" the icon `lock-open`.
- **The named manager and the last manager.** "Edit" and "Disable" are
  disabled on their rows. On hover, each shows the message of
  [Edit person](#edit-person) for that person.
- **State.** "enabled" shows as done, and "disabled" as cancelled
  ([Status](ui_patterns.md#status)).

### Edit person

- **Layout.** A dialog ([Editing](ui_patterns.md#editing)).
- **Fields.** "Role", a choice of the roles ([Roles](#roles)), in a `select`.
- **Refusals.** The messages are as below, above the buttons. A tool that
  disables such a person gets the same message.

  | Refusal           | Message                            |
  | ----------------- | ---------------------------------- |
  | the named manager | "The `manager` setting names NAME." |
  | the last manager  | "NAME is the last manager."        |

### Disable person

- **Layout.** A confirmation ([Confirming](ui_patterns.md#confirming)):
  "Disable NAME? This signs them out and ends their authorizations."

### Authorizations

- **Address.** `/authorizations`.
- **Layout.** The table "Authorizations"
  ([Tables](ui_patterns.md#tables)). The screen has no breadcrumbs.
- **The table.** A row per authorization of the reader, newest first. Its
  card's icon is `key`.

  | Column        | Holds                                     |
  | ------------- | ----------------------------------------- |
  | Client        | the client's name, or "an unnamed client" |
  | Sends back to | the host of the client's redirect URI     |
  | Allowed       | when the authorization began              |
  | Ends          | when the authorization ends               |

- **A row's buttons.** "End", with the icon `link-slash`.

### End authorization

- **Layout.** A confirmation ([Confirming](ui_patterns.md#confirming)):
  "End the authorization of NAME? NAME can no longer act as you."

## Out of scope

- Inviting people.
- Linking several identities to one person.
- Deleting a person; disabling them stands in for it.
- Naming who asked in what Linewright posts on GitHub.
- Commands that change people or end authorizations.
- Signing in for the commands; who can open the socket can use them
  ([Monitoring commands](operation.md#monitoring-commands)).
- Seeing or ending another person's authorizations; disabling the person
  ends them all.
- Direction, not the first version: identity providers other than Google
  and GitHub.
- Direction, not the first version: version 2026-07-28 of the protocol.
