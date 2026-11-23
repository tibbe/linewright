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
  or until the person signs out. A request for a screen from a browser that
  is not signed in goes to Sign in, and back to the screen after the
  sign-in.
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
serves the UI ([Serving the UI](operation.md#serving-the-ui)). It serves the
versions of the protocol from 2025-06-18 on.

- **Tools.** One tool per action ([Actions](#actions)), with the action as
  its title. A tool that only shows something is marked read-only.
- **Arguments.** A tool takes the ID of the thing that it shows or acts on,
  such as `run_id` or `person_id`. It also takes the filters of its screen
  and the fields of its dialog. Each argument has its name on the screen, in
  snake case.
- **What a tool shows.** A tool that shows something returns what its screen
  shows at the time of the call. The result is structured content with an
  output schema, and the same JSON as text. A fact that a command's `--json`
  also gives has the name it has there ([Output](operation.md#output)).
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
- **A refused call.** It returns a tool error with the message that the UI
  shows for the refusal.

## The authorization server

Linewright is its own OAuth 2.1 authorization server, and the MCP server is
the one resource it guards. A client is an app that acts as a person over
MCP, such as their coding agent. An authorization is a person's consent for
one client to act as them.

- **Issuer.** The public URL (`public-url`) is the issuer. Linewright builds
  every URL it gives out from it.
- **Endpoints.** Under the public URL: `/oauth/authorize` to authorize,
  `/oauth/token` for tokens and `/oauth/register` to register.
- **Resource metadata.** Linewright serves protected resource metadata (RFC
  9728) for the MCP server at `/.well-known/oauth-protected-resource/mcp`.
  It names Linewright as the authorization server.
- **Server metadata.** Linewright serves authorization server metadata (RFC
  8414) at `/.well-known/oauth-authorization-server`.
- **No token.** A request to the MCP server without a valid access token
  gets 401. Its `WWW-Authenticate` header points to the protected resource
  metadata.
- **Clients.** Every client is a public client. A client registers with an
  OAuth client ID metadata document
  (`draft-ietf-oauth-client-id-metadata-document`), whose URL is its client
  ID, or by dynamic client registration (RFC 7591). Linewright registers a
  client that asks for a client secret as a public client, without a secret.
- **Redirect URIs.** Each is HTTPS, or HTTP to a loopback address, and
  Linewright refuses a registration with any other. An authorization
  request's redirect URI must be one that its client registered, exactly.
- **Grants.** The authorization code grant, with PKCE and the `S256` method,
  and the refresh token grant. Linewright refuses every other grant and
  method.
- **Resource.** Linewright issues each token for the MCP server's URI,
  `/mcp` under the public URL (RFC 8707). It refuses a request for a token
  for another resource.
- **An authorization's ID.** A number that Linewright gives in the order
  that people allow clients, from 1.
- **How long it lasts.** An authorization lasts 30 days from its sign-in. It
  ends earlier when its person ends it ([Actions](#actions)), or when a
  client uses a refresh token that has stopped working ([Tokens](#tokens)).
  Signing out in the browser leaves it in place.
- **Logged.** Linewright's log records each client that registers, and each
  consent that a person gives or denies, with the person and the client.

1. The client registers.
2. The client sends the person to Linewright to authorize it.
3. The person signs in through an identity provider
   ([Signing in](#signing-in)).
4. The person allows the client on [Consent](#consent).
5. Linewright sends the person back to the client with a code.
6. The client exchanges the code, with its PKCE verifier, for an access
   token and a refresh token.

Each authorization takes steps 3 and 4, whatever the browser is signed in
to.

- **2a. Linewright cannot send the person back.** The request's client is
  unknown, or its redirect URI is not one that the client registered.
  Linewright does not redirect, and shows
  [Authorization error](#authorization-error).
- **3a. Linewright refuses the sign-in.** Sign in says why. The client gets
  no code.
- **4a. The person denies the client.** Linewright sends the person back to
  the client with the `access_denied` error.

## Tokens

A client's tokens act as the person who authorized it.

| Token         | Stops working                                                                       |
| ------------- | ----------------------------------------------------------------------------------- |
| access token  | one hour after Linewright issues it, or when its authorization ends                 |
| refresh token | when the client first uses a token that replaced it, or when its authorization ends |

- **Form.** A token is an opaque string.
- **Rotation.** Each use of a refresh token issues a new access token and a
  new refresh token, in the same authorization.
- **A lost response.** A client that did not get its new tokens uses the
  refresh token again, and gets new tokens again.
- **A refresh token that has stopped working.** Using one ends its
  authorization.

## Screens

| Screen              | Reached from                                                                        | Purpose                                      |
| ------------------- | ----------------------------------------------------------------------------------- | -------------------------------------------- |
| Sign in             | a screen opened without a sign-in, a client's authorization request, and "Sign out" | choose an identity provider to sign in with  |
| Consent             | Sign in, in a client's authorization request                                        | allow or deny the client                     |
| Authorization error | a client's authorization request that Linewright cannot send back                   | see why Linewright stopped                   |
| People              | the navigation ([Shell](shell.md))                                                  | see the people, and as a manager change them |
| Edit person         | "Edit" on a person, on People                                                       | change the person's role                     |
| Disable person      | "Disable" on a person, on People                                                    | confirm disabling the person                 |
| Authorizations      | "Authorizations" in the account's menu ([Shell](shell.md))                          | see one's own authorizations, and end one    |
| End authorization   | "End" on an authorization, on Authorizations                                        | confirm ending the authorization             |

### Sign in

- **Layout.** A card in the middle of the page. The card holds the heading,
  then the message, where there is one, then the identity providers.
- **Heading.** "Sign in to Linewright", and under it the repository's name.
- **The message.** It says why the person is on the screen, where there is a
  reason:

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
  Google" and "Sign in with GitHub", each with its provider's logo.

### Consent

- **Layout.** A card in the middle of the page, as on Sign in. The card
  holds the heading, then the facts, then "Allow" and "Deny".
- **Heading.** "Allow NAME to act as you?", with the client's name as it
  registered it, or "an unnamed client" without one.
- **Facts.**

  | Fact              | Holds                                 |
  | ----------------- | ------------------------------------- |
  | Signed in as      | the person's identity                 |
  | Sends you back to | the host of the client's redirect URI |
  | Access            | the person's role                     |

### Authorization error

- **Layout.** A card in the middle of the page, as on Sign in. The card
  holds the heading "Linewright cannot authorize this client", then the
  message.
- **The message.** "Linewright does not know this client." for an unknown
  client, and "This is not a redirect URI of this client." for a redirect
  URI that the client did not register.

### People

- **Layout.** The table "People" ([Tables](design_system.md#tables)).
- **The table.** A row per person ([People](#people)), in the order of their
  names by the Unicode Collation Algorithm.

  | Column    | Holds                                                                                 |
  | --------- | ------------------------------------------------------------------------------------- |
  | Person    | their avatar, then their name, then their identity provider's logo and their identity |
  | Role      | their role                                                                            |
  | State     | "enabled" or "disabled"                                                               |
  | Signed in | when they last signed in                                                              |

- **A row's buttons.** "Edit" ([Editing](design_system.md#editing)), then
  "Disable" on an enabled person and "Enable" on a disabled one.
- **The named manager and the last manager.** "Edit" and "Disable" are
  disabled on their rows. On hover, each shows the message of
  [Edit person](#edit-person) for that person.
- **State.** "enabled" shows as done, and "disabled" as cancelled
  ([Status](design_system.md#status)).

### Edit person

- **Layout.** A dialog ([Editing](design_system.md#editing)).
- **Fields.** "Role", a choice of the roles ([Roles](#roles)).
- **Refusals.** The messages are as below, above the buttons. A tool that
  disables such a person gets the same message.

  | Refusal           | Message                            |
  | ----------------- | ---------------------------------- |
  | the named manager | "The `manager` setting names NAME." |
  | the last manager  | "NAME is the last manager."        |

### Disable person

- **Layout.** A confirmation ([Confirming](design_system.md#confirming)):
  "Disable NAME? This signs them out and ends their authorizations."

### Authorizations

- **Layout.** The table "Authorizations"
  ([Tables](design_system.md#tables)). The screen has no breadcrumbs.
- **The table.** A row per authorization of the reader, newest first.

  | Column        | Holds                                     |
  | ------------- | ----------------------------------------- |
  | Client        | the client's name, or "an unnamed client" |
  | Sends back to | the host of the client's redirect URI     |
  | Allowed       | when the reader allowed the client        |
  | Ends          | when the authorization ends               |

- **A row's buttons.** "End".

### End authorization

- **Layout.** A confirmation ([Confirming](design_system.md#confirming)):
  "End the authorization of NAME? NAME can no longer act as you."

## Out of scope

- Inviting people.
- Linking several identities to one person.
- Deleting a person; disabling them stands in for it.
- Naming who asked in what Linewright posts on GitHub.
- Commands that change people or end authorizations.
- Signing in for the commands; who can open the socket can use them
  ([Monitoring commands](operation.md#monitoring-commands)).
- A token with less access than its person's role.
- Seeing or ending another person's authorizations; disabling the person
  ends them all.
- Direction, not the first version: identity providers other than Google
  and GitHub.
