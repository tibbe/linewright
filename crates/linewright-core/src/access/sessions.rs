//! A browser's sign-in (`docs/specs/access.md`, Signing in, Signed in): an
//! opaque cookie, kept in the database only as its SHA-256, for 30 days or
//! until the person signs out.

use anyhow::Context as _;
use chrono::TimeDelta;

use super::Caller;
use super::people::{People, Person, PersonId, State};
use super::tokens::{hashed, host_cookie, random_token};
use crate::database::{Database, parse_time, time_text};
use crate::domain::Timestamp;

/// The name of the cookie that holds a browser's sign-in.
pub const SESSION_COOKIE: &str = "__Host-linewright-session";

/// How long a browser stays signed in.
pub const SIGNED_IN_FOR: TimeDelta = TimeDelta::days(30);

/// How long Linewright knows a sign-in after it ended, and a browser keeps
/// its cookie after that, so that Sign in can say that it ended
/// (`docs/specs/access.md`, Sign in, The message).
pub const KNOWN_ENDED_FOR: TimeDelta = TimeDelta::days(30);

/// The browsers' sign-ins, in the database.
#[derive(Clone, Debug)]
pub struct Sessions {
    db: Database,
}

/// The value of a browser's session cookie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionCookie(String);

impl SessionCookie {
    /// The cookie whose value is `value`, as a browser sent it.
    pub fn of(value: String) -> SessionCookie {
        SessionCookie(value)
    }

    /// Its value.
    pub fn value(&self) -> &str {
        &self.0
    }

    /// The `Set-Cookie` that keeps it in the browser for as long as
    /// Linewright knows its sign-in: [`SIGNED_IN_FOR`], then
    /// [`KNOWN_ENDED_FOR`].
    pub fn set_cookie(&self) -> String {
        host_cookie(SESSION_COOKIE, &self.0, SIGNED_IN_FOR + KNOWN_ENDED_FOR)
    }
}

/// Who a browser's session cookie signs in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resolved {
    Caller(Caller),
    /// A sign-in that ended, for the reason.
    Ended(Reason),
    /// No sign-in that Linewright knows of: an unknown or forgotten cookie,
    /// one of a person no longer in its record, or one whose sign-in
    /// disabling ended and whose person is enabled again.
    NoOne,
}

impl Resolved {
    /// The caller, if it signs one in.
    pub fn caller(self) -> Option<Caller> {
        match self {
            Resolved::Caller(caller) => Some(caller),
            Resolved::Ended(_) | Resolved::NoOne => None,
        }
    }
}

/// Why a browser's sign-in ended (`docs/specs/access.md`, Sign in, The
/// message).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    SignedOut,
    /// Its 30 days ended.
    Expired,
    /// Its person is disabled, with their identity as it is shown.
    Disabled(String),
}

impl Sessions {
    pub fn new(db: Database) -> Sessions {
        Sessions { db }
    }

    /// Signs a browser in as `person` from `now` for [`SIGNED_IN_FOR`],
    /// forgetting the sign-ins that ended over [`KNOWN_ENDED_FOR`] before.
    /// A person disabled by then is signed in nowhere: the cookie signs no
    /// one in, even once they are enabled again.
    pub async fn start(&self, person: PersonId, now: Timestamp) -> anyhow::Result<SessionCookie> {
        let cookie = SessionCookie(random_token());
        sqlx::query("DELETE FROM browser_sessions WHERE expires_at < ?")
            .bind(time_text(now - KNOWN_ENDED_FOR))
            .execute(self.db.pool())
            .await
            .context("forgetting the ended sign-ins")?;
        sqlx::query(
            "INSERT INTO browser_sessions (hash, person, expires_at)
             SELECT ?, id, ? FROM people WHERE id = ? AND state = 'enabled'",
        )
        .bind(hashed(&cookie.0))
        .bind(time_text(now + SIGNED_IN_FOR))
        .bind(person.row())
        .execute(self.db.pool())
        .await
        .with_context(|| format!("signing person {person} in"))?;
        Ok(cookie)
    }

    /// Who `cookie` signs in at `now`, as their record has them now, or
    /// why its sign-in ended.
    pub async fn resolve(
        &self,
        cookie: &SessionCookie,
        now: Timestamp,
    ) -> anyhow::Result<Resolved> {
        let Some(session) = self.session(&hashed(&cookie.0)).await? else {
            return Ok(Resolved::NoOne);
        };
        let person = People::new(self.db.clone()).get(session.person).await?;
        Ok(session.resolved(person, now))
    }

    /// Signs out the browser of `cookie` at `now`, and gives who it signed
    /// in, if it still signed them in. Linewright knows the sign-in for
    /// [`KNOWN_ENDED_FOR`] after, so that each tab of the browser can say
    /// why it ended.
    pub async fn end(
        &self,
        cookie: &SessionCookie,
        now: Timestamp,
    ) -> anyhow::Result<Option<Person>> {
        let person: Option<i64> = sqlx::query_scalar(ending!("hash = ? RETURNING person"))
            .bind(Ended::SignedOut.value())
            .bind(time_text(now))
            .bind(time_text(now))
            .bind(hashed(&cookie.0))
            .fetch_optional(self.db.pool())
            .await
            .context("signing the browser out")?;
        match person {
            Some(person) => {
                People::new(self.db.clone())
                    .get(PersonId::of_row(person))
                    .await
            }
            None => Ok(None),
        }
    }

    /// The sign-in of `hash`, if Linewright knows it.
    async fn session(&self, hash: &[u8]) -> anyhow::Result<Option<Session>> {
        let row: Option<(i64, String, Option<String>)> =
            sqlx::query_as("SELECT person, expires_at, ended FROM browser_sessions WHERE hash = ?")
                .bind(hash)
                .fetch_optional(self.db.pool())
                .await
                .context("reading the browser's sign-in")?;
        let Some((person, ends_at, ended)) = row else {
            return Ok(None);
        };
        Ok(Some(Session {
            person: PersonId::of_row(person),
            ends_at: parse_time(&ends_at)?,
            ended: ended.as_deref().map(Ended::of).transpose()?,
        }))
    }
}

/// Ends, at `now`, every sign-in of `person` that still signs them in, as
/// disabling them does (`docs/specs/access.md`, People, Disabling).
pub(crate) async fn end_all_of(
    conn: &mut sqlx::SqliteConnection,
    person: PersonId,
    now: Timestamp,
) -> anyhow::Result<()> {
    sqlx::query(ending!("person = ?"))
        .bind(Ended::Disabled.value())
        .bind(time_text(now))
        .bind(time_text(now))
        .bind(person.row())
        .execute(conn)
        .await
        .with_context(|| format!("signing person {person} out of every browser"))?;
    Ok(())
}

/// The statement that ends at a time each sign-in that `$which` picks and
/// that still signs someone in then, saying why. It binds why, the time
/// twice, then what `$which` binds.
macro_rules! ending {
    ($which:literal) => {
        concat!(
            "UPDATE browser_sessions SET ended = ?, expires_at = ?
             WHERE ended IS NULL AND expires_at > ? AND ",
            $which
        )
    };
}
use ending;

/// A browser's sign-in, as the database keeps it.
struct Session {
    person: PersonId,
    /// When it ends, or ended.
    ends_at: Timestamp,
    /// Why it ended before its 30 days, if it did.
    ended: Option<Ended>,
}

/// Why a sign-in ended before its 30 days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    SignedOut,
    Disabled,
}

impl Ended {
    /// Its value in the database.
    fn value(self) -> &'static str {
        match self {
            Ended::SignedOut => "signed-out",
            Ended::Disabled => "disabled",
        }
    }

    fn of(value: &str) -> anyhow::Result<Ended> {
        [Ended::SignedOut, Ended::Disabled]
            .into_iter()
            .find(|ended| ended.value() == value)
            .with_context(|| format!("a sign-in ended as {value:?}"))
    }
}

impl Session {
    /// Who it signs in at `now`, with `person` as their record has them
    /// now, or why it ended: why it ended first, and a sign-in Linewright
    /// no longer knows as none.
    fn resolved(&self, person: Option<Person>, now: Timestamp) -> Resolved {
        if self.ends_at + KNOWN_ENDED_FOR < now {
            return Resolved::NoOne;
        }
        let disabled = |found: &Person| found.state == State::Disabled;
        match (self.ended, person) {
            (Some(Ended::SignedOut), _) => Resolved::Ended(Reason::SignedOut),
            (Some(Ended::Disabled), Some(person)) if disabled(&person) => {
                Resolved::Ended(Reason::Disabled(person.shown_as))
            }
            (Some(Ended::Disabled), _) | (None, None) => Resolved::NoOne,
            (None, Some(_)) if self.ends_at <= now => Resolved::Ended(Reason::Expired),
            (None, Some(person)) if disabled(&person) => {
                Resolved::Ended(Reason::Disabled(person.shown_as))
            }
            (None, Some(person)) => Caller::of(person).map_or(Resolved::NoOne, Resolved::Caller),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Role;
    use crate::access::people::tests::person;
    use crate::access::people::{Claims, Vouched};

    /// `docs/specs/access.md`, Signing in, Signed in and An open screen;
    /// Sign in, The message.
    #[test_log::test]
    fn a_sign_in_resolves_to_its_person_or_why_it_ended_while_linewright_knows_it() {
        let now = chrono::DateTime::UNIX_EPOCH + TimeDelta::days(100);
        let (enabled, disabled) = (
            person(1, Role::Viewer, State::Enabled),
            person(1, Role::Viewer, State::Disabled),
        );
        let kept = |ends_at, ended| Session {
            person: PersonId::of_row(1),
            ends_at,
            ended,
        };
        let (live, expired) = (now + TimeDelta::days(1), now);
        let forgotten = now - KNOWN_ENDED_FOR - TimeDelta::seconds(1);
        let shown_as = || Reason::Disabled("person1@example.com".into());
        for (session, person, resolved, case) in [
            (kept(live, None), Some(&enabled), None, "signed in"),
            (
                kept(live, None),
                Some(&disabled),
                Some(Resolved::Ended(shown_as())),
                "disabled",
            ),
            (kept(live, None), None, Some(Resolved::NoOne), "gone"),
            (
                kept(expired, None),
                Some(&disabled),
                Some(Resolved::Ended(Reason::Expired)),
                "expired, then disabled",
            ),
            (
                kept(expired, Some(Ended::SignedOut)),
                Some(&disabled),
                Some(Resolved::Ended(Reason::SignedOut)),
                "signed out, then disabled",
            ),
            (
                kept(expired, Some(Ended::Disabled)),
                Some(&disabled),
                Some(Resolved::Ended(shown_as())),
                "ended by disabling",
            ),
            (
                kept(expired, Some(Ended::Disabled)),
                Some(&enabled),
                Some(Resolved::NoOne),
                "ended by disabling, then enabled",
            ),
            (
                kept(forgotten, Some(Ended::SignedOut)),
                Some(&enabled),
                Some(Resolved::NoOne),
                "forgotten",
            ),
        ] {
            let resolved = resolved.unwrap_or_else(|| {
                Resolved::Caller(Caller::of(enabled.clone()).expect("an enabled caller"))
            });
            assert_eq!(session.resolved(person.cloned(), now), resolved, "{case}");
        }
    }

    /// `docs/specs/access.md`, Signing in, Signed in and An open screen;
    /// People, Disabling.
    #[test_log::test(tokio::test)]
    async fn signing_out_or_disabling_ends_only_a_sign_in_that_still_signs_someone_in() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let start = chrono::DateTime::UNIX_EPOCH;
        let claims = Claims {
            subject: "1".into(),
            shown_as: "ada@example.com".into(),
            name: None,
            avatar: None,
            vouched: Vouched::Google {
                email_verified: true,
            },
        };
        let ada = People::new(db.clone())
            .add(&claims, Role::Viewer, start)
            .await
            .unwrap();
        let sessions = Sessions::new(db.clone());
        let signed_out = sessions.start(ada.id, start).await.unwrap();
        let expired = sessions.start(ada.id, start - SIGNED_IN_FOR).await.unwrap();
        let disabled = sessions.start(ada.id, start).await.unwrap();

        let ended = sessions.end(&signed_out, start).await.unwrap();
        let again = sessions.end(&signed_out, start).await.unwrap();
        let too_late = sessions.end(&expired, start).await.unwrap();
        let mut conn = db.pool().acquire().await.unwrap();
        end_all_of(&mut conn, ada.id, start).await.unwrap();
        drop(conn);

        assert_eq!(ended.map(|person| person.id), Some(ada.id));
        assert_eq!((again, too_late), (None, None));
        let ended_as = async |cookie: &SessionCookie| {
            let session = sessions.session(&hashed(&cookie.0)).await.unwrap().unwrap();
            (session.ends_at, session.ended)
        };
        assert_eq!(ended_as(&signed_out).await, (start, Some(Ended::SignedOut)));
        assert_eq!(ended_as(&expired).await, (start, None));
        assert_eq!(ended_as(&disabled).await, (start, Some(Ended::Disabled)));
    }
}
