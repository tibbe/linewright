//! The people Linewright signs in (`docs/specs/access.md`, People), and how
//! it decides each sign-in (`docs/specs/access.md`, Signing in).

use std::fmt;

use anyhow::Context as _;
use url::Url;

use super::Role;
use super::providers::{Provider, Providers, SignUpDomains, github_login};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;
use crate::host::domain_name;

/// The identity the `manager` setting names, of a provider Linewright
/// offers: lower-cased, as Linewright compares it ignoring case
/// (`docs/specs/access.md`, Signing in, Matching an identity).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerName {
    provider: Provider,
    /// The email for Google, the login for GitHub.
    identity: String,
}

/// Why a `manager` setting is no [`ManagerName`].
#[derive(Debug, PartialEq, Eq)]
pub enum NotAManager {
    /// It names no identity, with why.
    Malformed(&'static str),
    /// It names an identity of this provider, which Linewright does not
    /// offer.
    NotOffered(Provider),
}

impl ManagerName {
    /// The identity `name` names, `google:EMAIL` or `github:LOGIN`, the
    /// provider ignoring case, when it is of one of `providers`.
    pub fn parse(name: &str, providers: &Providers) -> Result<ManagerName, NotAManager> {
        let (provider, identity) = written(name).map_err(NotAManager::Malformed)?;
        if providers.offer(provider) {
            Ok(ManagerName { provider, identity })
        } else {
            Err(NotAManager::NotOffered(provider))
        }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// The email for Google, the login for GitHub, lower-cased.
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Whether it names the identity of `provider` shown as `shown_as`,
    /// ignoring case.
    fn names(&self, provider: Provider, shown_as: &str) -> bool {
        self.provider == provider && self.identity.eq_ignore_ascii_case(shown_as)
    }
}

/// The provider and the identity, lower-cased, that `name` writes, or why it
/// writes none.
fn written(name: &str) -> Result<(Provider, String), &'static str> {
    const FORM: &str = "it is `google:` and an email, or `github:` and a login";
    let (provider, identity) = name.split_once(':').ok_or(FORM)?;
    if provider.eq_ignore_ascii_case("google") {
        let email = identity
            .split_once('@')
            .filter(|(local, _)| !local.is_empty() && !local.contains(char::is_whitespace))
            .and_then(|(local, domain)| Some(format!("{local}@{}", domain_name(domain)?)))
            .ok_or("`google:` is followed by an email, such as `google:alice@example.com`")?;
        Ok((Provider::Google, email.to_ascii_lowercase()))
    } else if provider.eq_ignore_ascii_case("github") {
        let login = github_login(identity)
            .ok_or("`github:` is followed by a login, such as `github:octocat`")?;
        Ok((Provider::GitHub, login))
    } else {
        Err(FORM)
    }
}

impl fmt::Display for ManagerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider, self.identity)
    }
}

/// A person's ID (`docs/specs/access.md`, People, A person's ID).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PersonId(i64);

impl PersonId {
    pub(crate) fn row(self) -> i64 {
        self.0
    }

    pub(crate) fn of_row(row: i64) -> PersonId {
        PersonId(row)
    }
}

impl fmt::Display for PersonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Who a person is: an identity provider and the subject it gives them
/// (`docs/specs/access.md`, People, Who a person is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub provider: Provider,
    pub subject: String,
}

/// Whether a person may sign in (`docs/specs/access.md`, People, State).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Enabled,
    Disabled,
}

impl State {
    /// The state named `name`, as the database keeps it.
    fn named(name: &str) -> Option<State> {
        match name {
            "enabled" => Some(State::Enabled),
            "disabled" => Some(State::Disabled),
            _ => None,
        }
    }
}

/// A person in Linewright's record (`docs/specs/access.md`, People).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: PersonId,
    pub identity: Identity,
    /// Their identity as it is shown: the verified email for Google, the
    /// login for GitHub, as of their last sign-in.
    pub shown_as: String,
    pub name: Option<String>,
    pub avatar: Option<Url>,
    pub role: Role,
    pub state: State,
    pub signed_in_at: Timestamp,
}

impl Person {
    /// What they are shown by: their name, or else their identity
    /// (`docs/specs/access.md`, People, How a person is shown).
    pub fn shown_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.shown_as)
    }

    /// What stands in for their avatar: the first letter of what they are
    /// shown by, upper-cased.
    pub fn letter(&self) -> String {
        self.shown_name()
            .chars()
            .find(|c| c.is_alphanumeric())
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }

    /// Their identity as the log gives it, as `manager` names one, such as
    /// `google:ada@example.com`.
    pub fn logged(&self) -> String {
        logged(self.identity.provider, &self.shown_as)
    }
}

/// The identity of `provider` shown as `shown_as`, as the log gives it.
fn logged(provider: Provider, shown_as: &str) -> String {
    format!("{provider}:{shown_as}")
}

/// What an identity provider says of the person it signed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Claims {
    pub identity: Identity,
    /// The email for Google, the login for GitHub.
    pub shown_as: String,
    pub name: Option<String>,
    pub avatar: Option<Url>,
    /// Whether the provider verified the email it shows them as.
    pub email_verified: bool,
}

impl Claims {
    /// The identity as the log gives it, as [`Person::logged`] does.
    pub fn logged(&self) -> String {
        logged(self.identity.provider, &self.shown_as)
    }
}

/// An identity provider's sign-up rule (`docs/specs/access.md`, Signing
/// in, The sign-up rule).
#[derive(Clone, Copy, Debug)]
pub enum SignUpRule<'a> {
    /// The verified email's domain is one of these.
    Google(&'a SignUpDomains),
}

impl SignUpRule<'_> {
    /// Whether `claims` meet it.
    pub fn meets(&self, claims: &Claims) -> bool {
        match self {
            SignUpRule::Google(domains) => claims
                .shown_as
                .rsplit_once('@')
                .and_then(|(_, domain)| domain_name(domain))
                .is_some_and(|domain| domains.domains().contains(&domain)),
        }
    }
}

/// What Linewright does with a sign-in (`docs/specs/access.md`, Signing in,
/// 3a to 3e).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignInDecision {
    Refuse(Refusal),
    /// Adds the identity as a person of `role`, and signs them in.
    Add {
        role: Role,
    },
    /// Signs in `person`, first making them an enabled manager where
    /// `make_manager` says so.
    SignIn {
        person: PersonId,
        make_manager: bool,
    },
}

/// Why Linewright refuses a sign-in, with what Sign in says of it
/// (`docs/specs/access.md`, Sign in, The message).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// 3a, with the email.
    #[error("Google has not verified {0}.")]
    Unverified(String),
    /// 3c, with the identity as it is shown.
    #[error("{0} has no access to this Linewright.")]
    NoAccess(String),
    /// 3e, with the identity as it is shown.
    #[error("{0} is disabled.")]
    Disabled(String),
}

/// What Linewright does with the sign-in of `claims`, under the provider's
/// sign-up rule `rule` and the `manager` setting `manager`, for the person
/// `person` with that identity, if there is one: the first of 3a to 3e that
/// applies (`docs/specs/access.md`, Signing in).
pub fn decide_sign_in(
    claims: &Claims,
    rule: &SignUpRule,
    manager: Option<&ManagerName>,
    person: Option<&Person>,
) -> SignInDecision {
    if !claims.email_verified {
        return SignInDecision::Refuse(Refusal::Unverified(claims.shown_as.clone()));
    }
    if manager.is_some_and(|manager| manager.names(claims.identity.provider, &claims.shown_as)) {
        return match person {
            None => SignInDecision::Add {
                role: Role::Manager,
            },
            Some(person) => SignInDecision::SignIn {
                person: person.id,
                make_manager: person.role != Role::Manager || person.state != State::Enabled,
            },
        };
    }
    if !rule.meets(claims) {
        return SignInDecision::Refuse(Refusal::NoAccess(claims.shown_as.clone()));
    }
    match person {
        None => SignInDecision::Add { role: Role::Viewer },
        Some(person) if person.state == State::Disabled => {
            SignInDecision::Refuse(Refusal::Disabled(claims.shown_as.clone()))
        }
        Some(person) => SignInDecision::SignIn {
            person: person.id,
            make_manager: false,
        },
    }
}

const READING: &str = "reading the people";

/// Linewright's record of people, in its database.
#[derive(Clone, Debug)]
pub struct People {
    db: Database,
}

/// A row of `people`, as [`People`] reads it.
type Row = (
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
);

/// The columns of `people` that a [`Row`] holds.
macro_rules! people_columns {
    () => {
        "people.id, people.provider, people.subject, people.shown_as, people.name, \
         people.avatar, people.role, people.state, people.signed_in_at"
    };
}

impl People {
    pub fn new(db: Database) -> People {
        People { db }
    }

    /// The person whose identity is `identity`, if there is one.
    pub async fn find_by_identity(&self, identity: &Identity) -> anyhow::Result<Option<Person>> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            people_columns!(),
            " FROM people WHERE provider = ? AND subject = ?"
        ))
        .bind(identity.provider.to_string())
        .bind(&identity.subject)
        .fetch_optional(self.db.pool())
        .await
        .context(READING)?;
        row.map(person_of).transpose()
    }

    /// The person `id`, if there is one.
    pub async fn get(&self, id: PersonId) -> anyhow::Result<Option<Person>> {
        let row: Option<Row> = sqlx::query_as(concat!(
            "SELECT ",
            people_columns!(),
            " FROM people WHERE id = ?"
        ))
        .bind(id.row())
        .fetch_optional(self.db.pool())
        .await
        .context(READING)?;
        row.map(person_of).transpose()
    }

    /// Adds the enabled person of `claims` with `role`, signed in at `now`;
    /// or, where another sign-in of theirs has just added them, records
    /// this sign-in as [`People::record_sign_in`] does.
    pub async fn add(&self, claims: &Claims, role: Role, now: Timestamp) -> anyhow::Result<Person> {
        let row: Row = sqlx::query_as(concat!(
            "INSERT INTO people
                (provider, subject, shown_as, name, avatar, role, state, signed_in_at)
             VALUES (?, ?, ?, ?, ?, ?, 'enabled', ?)
             ON CONFLICT (provider, subject) DO UPDATE SET shown_as = excluded.shown_as,
                name = excluded.name, avatar = excluded.avatar,
                signed_in_at = excluded.signed_in_at
             RETURNING ",
            people_columns!(),
            ""
        ))
        .bind(claims.identity.provider.to_string())
        .bind(&claims.identity.subject)
        .bind(&claims.shown_as)
        .bind(&claims.name)
        .bind(claims.avatar.as_ref().map(Url::as_str))
        .bind(role.name())
        .bind(time_text(now))
        .fetch_one(self.db.pool())
        .await
        .with_context(|| format!("adding {}", claims.logged()))?;
        person_of(row)
    }

    /// Records the sign-in of the person `id` with `claims` at `now`: their
    /// name, avatar and identity shown as `claims` give them, and, where
    /// `make_manager` says so, the role of manager and enabled.
    pub async fn record_sign_in(
        &self,
        id: PersonId,
        claims: &Claims,
        make_manager: bool,
        now: Timestamp,
    ) -> anyhow::Result<Person> {
        let row: Row = sqlx::query_as(concat!(
            "UPDATE people SET shown_as = ?, name = ?, avatar = ?, signed_in_at = ?,
                role = CASE WHEN ? THEN 'manager' ELSE role END,
                state = CASE WHEN ? THEN 'enabled' ELSE state END
             WHERE id = ?
             RETURNING ",
            people_columns!(),
            ""
        ))
        .bind(&claims.shown_as)
        .bind(&claims.name)
        .bind(claims.avatar.as_ref().map(Url::as_str))
        .bind(time_text(now))
        .bind(make_manager)
        .bind(make_manager)
        .bind(id.row())
        .fetch_one(self.db.pool())
        .await
        .with_context(|| format!("recording the sign-in of {}", claims.logged()))?;
        person_of(row)
    }

    /// Makes each person whose identity matched `manager` at their last
    /// sign-in an enabled manager (`docs/specs/access.md`, People, The named
    /// manager), and gives them.
    pub async fn at_startup(&self, manager: &ManagerName) -> anyhow::Result<Vec<Person>> {
        let rows: Vec<Row> = sqlx::query_as(concat!(
            "SELECT ",
            people_columns!(),
            " FROM people WHERE provider = ?"
        ))
        .bind(manager.provider().to_string())
        .fetch_all(self.db.pool())
        .await
        .context(READING)?;
        let mut made = vec![];
        for person in rows.into_iter().map(person_of) {
            let person = person?;
            let named = manager.names(person.identity.provider, &person.shown_as);
            if named && (person.role != Role::Manager || person.state != State::Enabled) {
                let row: Row = sqlx::query_as(concat!(
                    "UPDATE people SET role = 'manager', state = 'enabled' WHERE id = ? RETURNING ",
                    people_columns!(),
                    ""
                ))
                .bind(person.id.row())
                .fetch_one(self.db.pool())
                .await
                .with_context(|| format!("making {manager} a manager"))?;
                made.push(person_of(row)?);
            }
        }
        Ok(made)
    }
}

/// The person a row of `people` holds.
fn person_of(row: Row) -> anyhow::Result<Person> {
    let (id, provider, subject, shown_as, name, avatar, role, state, signed_in_at) = row;
    let provider = Provider::named(&provider)
        .with_context(|| format!("person {id} has the unknown provider {provider:?}"))?;
    let role =
        Role::named(&role).with_context(|| format!("person {id} has the unknown role {role:?}"))?;
    let state = State::named(&state)
        .with_context(|| format!("person {id} has the unknown state {state:?}"))?;
    Ok(Person {
        id: PersonId(id),
        identity: Identity { provider, subject },
        shown_as,
        name,
        avatar: avatar.and_then(|avatar| Url::parse(&avatar).ok()),
        role,
        state,
        signed_in_at: parse_time(&signed_in_at)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Person `id`, of `role` and `state`, signed in with Google as
    /// `person{id}@example.com`.
    pub(crate) fn person(id: i64, role: Role, state: State) -> Person {
        Person {
            id: PersonId(id),
            identity: Identity {
                provider: Provider::Google,
                subject: format!("10{id}"),
            },
            shown_as: format!("person{id}@example.com"),
            name: None,
            avatar: None,
            role,
            state,
            signed_in_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    fn claims(email: &str, verified: bool) -> Claims {
        Claims {
            identity: Identity {
                provider: Provider::Google,
                subject: "101".into(),
            },
            shown_as: email.into(),
            name: Some("Ada Lovelace".into()),
            avatar: None,
            email_verified: verified,
        }
    }

    #[track_caller]
    fn domains(domain: &str) -> SignUpDomains {
        SignUpDomains::of(domain).unwrap()
    }

    #[track_caller]
    fn manager(name: &str) -> ManagerName {
        let google = super::super::providers::Google::for_tests(domains("example.com"));
        ManagerName::parse(name, &Providers::Google(google)).unwrap()
    }

    /// Person 1, `ada@example.com`, of `role` and `state`.
    fn ada(role: Role, state: State) -> Person {
        Person {
            shown_as: "ada@example.com".into(),
            ..person(1, role, state)
        }
    }

    /// 3a comes first: an unverified email is refused even for the
    /// identity `manager` names.
    #[test_log::test]
    fn an_email_google_has_not_verified_is_refused() {
        let rule = domains("example.com");
        let named = manager("google:ada@example.com");

        let decided = decide_sign_in(
            &claims("ada@example.com", false),
            &SignUpRule::Google(&rule),
            Some(&named),
            None,
        );

        assert_eq!(
            decided,
            SignInDecision::Refuse(Refusal::Unverified("ada@example.com".into()))
        );
        assert_eq!(
            Refusal::Unverified("ada@example.com".into()).to_string(),
            "Google has not verified ada@example.com."
        );
    }

    /// 3b: the identity `manager` names, ignoring case, is a manager
    /// whatever the sign-up rule says.
    #[test_log::test]
    fn the_identity_manager_names_signs_in_as_a_manager_whatever_the_rule() {
        let rule = SignUpDomains::default();
        let named = manager("google:Ada@Example.com");
        let decide = |person: Option<&Person>| {
            decide_sign_in(
                &claims("ADA@example.com", true),
                &SignUpRule::Google(&rule),
                Some(&named),
                person,
            )
        };

        assert_eq!(
            decide(None),
            SignInDecision::Add {
                role: Role::Manager
            }
        );
        for (role, state, make_manager) in [
            (Role::Manager, State::Enabled, false),
            (Role::Viewer, State::Enabled, true),
            (Role::Contributor, State::Enabled, true),
            (Role::Manager, State::Disabled, true),
            (Role::Viewer, State::Disabled, true),
        ] {
            assert_eq!(
                decide(Some(&ada(role, state))),
                SignInDecision::SignIn {
                    person: PersonId(1),
                    make_manager
                },
                "{role:?} {state:?}"
            );
        }
    }

    /// 3c comes before 3d and 3e: a person keeps their record.
    #[test_log::test]
    fn an_identity_outside_the_sign_up_rule_is_refused() {
        let rule = domains("example.com");
        let named = manager("google:grace@example.com");

        for person in [
            None,
            Some(ada(Role::Manager, State::Enabled)),
            Some(ada(Role::Viewer, State::Disabled)),
        ] {
            assert_eq!(
                decide_sign_in(
                    &claims("ada@elsewhere.example", true),
                    &SignUpRule::Google(&rule),
                    Some(&named),
                    person.as_ref(),
                ),
                SignInDecision::Refuse(Refusal::NoAccess("ada@elsewhere.example".into())),
            );
        }
        assert_eq!(
            Refusal::NoAccess("ada@elsewhere.example".into()).to_string(),
            "ada@elsewhere.example has no access to this Linewright."
        );
    }

    /// The rule is on the email's domain alone, ignoring case: not a
    /// subdomain of one it names.
    #[test_log::test]
    fn the_sign_up_rule_is_the_emails_domain() {
        let rule = domains("Example.com").and(&domains("acme.test"));
        let meets = |email| SignUpRule::Google(&rule).meets(&claims(email, true));

        assert!(meets("ada@EXAMPLE.com"));
        assert!(meets("ada@acme.test"));
        assert!(!meets("ada@mail.example.com"));
        assert!(!meets("ada@example.com.evil.test"));
        assert!(!meets("example.com"));
    }

    #[test_log::test]
    fn a_new_identity_is_added_as_a_viewer() {
        let rule = domains("example.com");

        let decided = decide_sign_in(
            &claims("ada@example.com", true),
            &SignUpRule::Google(&rule),
            None,
            None,
        );

        assert_eq!(decided, SignInDecision::Add { role: Role::Viewer });
    }

    #[test_log::test]
    fn a_disabled_person_is_refused() {
        let rule = domains("example.com");
        let decide = |person: &Person| {
            decide_sign_in(
                &claims("ada@example.com", true),
                &SignUpRule::Google(&rule),
                None,
                Some(person),
            )
        };

        assert_eq!(
            decide(&ada(Role::Manager, State::Disabled)),
            SignInDecision::Refuse(Refusal::Disabled("ada@example.com".into()))
        );
        assert_eq!(
            decide(&ada(Role::Contributor, State::Enabled)),
            SignInDecision::SignIn {
                person: PersonId(1),
                make_manager: false
            }
        );
        assert_eq!(
            Refusal::Disabled("ada@example.com".into()).to_string(),
            "ada@example.com is disabled."
        );
    }

    /// `docs/specs/access.md`, People, How a person is shown.
    #[test_log::test]
    fn a_person_is_shown_by_their_name_or_else_their_identity() {
        let named = Person {
            name: Some("ada Lovelace".into()),
            ..person(1, Role::Viewer, State::Enabled)
        };
        let unnamed = person(2, Role::Viewer, State::Enabled);

        assert_eq!(
            (named.shown_name(), named.letter().as_str()),
            ("ada Lovelace", "A")
        );
        assert_eq!(
            (unnamed.shown_name(), unnamed.letter().as_str()),
            ("person2@example.com", "P")
        );
    }

    /// `docs/specs/access.md`, People, The named manager: at startup.
    #[test_log::test(tokio::test)]
    async fn at_startup_each_person_whose_identity_matches_manager_is_an_enabled_manager() {
        let dir = tempfile::tempdir().unwrap();
        let people = People::new(Database::open(dir.path()).await.unwrap());
        let now = chrono::DateTime::UNIX_EPOCH;
        let ada = people
            .add(&claims("Ada@example.com", true), Role::Viewer, now)
            .await
            .unwrap();
        let grace_claims = Claims {
            identity: Identity {
                provider: Provider::Google,
                subject: "102".into(),
            },
            ..claims("grace@example.com", true)
        };
        let grace = people.add(&grace_claims, Role::Viewer, now).await.unwrap();
        sqlx::query("UPDATE people SET state = 'disabled' WHERE id = ?")
            .bind(ada.id.row())
            .execute(people.db.pool())
            .await
            .unwrap();

        let made = people
            .at_startup(&manager("google:ada@EXAMPLE.com"))
            .await
            .unwrap();

        let ada = people.get(ada.id).await.unwrap().unwrap();
        assert_eq!(made, std::slice::from_ref(&ada));
        assert_eq!((ada.role, ada.state), (Role::Manager, State::Enabled));
        assert_eq!(people.get(grace.id).await.unwrap().unwrap(), grace);
    }
}
