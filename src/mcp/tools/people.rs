//! The tools on people (`docs/specs/access.md`, People, Edit person and
//! Disable person): seeing them, changing their roles, and disabling and
//! enabling them.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Context, Tool, id};
use crate::access::people::{ChangeRefusal, Person, PersonId, State};
use crate::access::{Level, Manage, Permit, Read, Role};
use crate::actions::{self, Action, Missing, PersonListed};
use crate::domain::Timestamp;
use crate::mcp::page::{Page, Paging};

/// A person, as a tool returns them.
#[derive(Serialize, JsonSchema)]
pub(super) struct PersonOut {
    id: i64,
    /// Their name as their identity provider gives it, if it gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Their identity provider: `google` or `github`.
    provider: &'static str,
    /// Their identity, as it is shown: the email for Google, the login for
    /// GitHub.
    identity: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar: Option<String>,
    role: Role,
    /// `enabled` or `disabled`.
    state: &'static str,
    /// When they last signed in.
    signed_in_at: Timestamp,
}

impl From<Person> for PersonOut {
    fn from(person: Person) -> PersonOut {
        PersonOut {
            id: person.id.row(),
            name: person.name,
            provider: person.identity.provider.key(),
            identity: person.shown_as,
            avatar: person.avatar.map(String::from),
            role: person.role,
            state: match person.state {
                State::Enabled => "enabled",
                State::Disabled => "disabled",
            },
            signed_in_at: person.signed_in_at,
        }
    }
}

/// A person as People lists them.
#[derive(Serialize, JsonSchema)]
pub(super) struct PersonListedOut {
    #[serde(flatten)]
    person: PersonOut,
    /// Why they cannot lose the role of manager or be disabled, if they
    /// cannot: what People shows a manager on their "Edit" and "Disable".
    #[serde(skip_serializing_if = "Option::is_none")]
    protected: Option<String>,
}

/// See the people, in the order of their names (`docs/specs/access.md`,
/// People).
#[derive(Deserialize, JsonSchema)]
#[serde(transparent)]
pub(in crate::mcp) struct ListPeople {
    paging: Paging,
}

impl Tool for ListPeople {
    const ACTION: Action = Action::ListPeople;
    type Level = Read;
    type Output = Page<PersonListedOut>;

    async fn run(
        self,
        permit: &Permit<Read>,
        context: &Context<'_>,
    ) -> Result<Page<PersonListedOut>, String> {
        let manages = permit.caller().can(Level::Manage);
        let listed = actions::list_people(permit, context.sources, context.manager)
            .await
            .map_err(|e| format!("{e:#}"))?;
        let listed = listed
            .into_iter()
            .map(|PersonListed { person, protected }| PersonListedOut {
                person: person.into(),
                protected: protected
                    .filter(|_| manages)
                    .map(|protected| protected.to_string()),
            })
            .collect();
        self.paging
            .page(listed, |listed| listed.person.id.to_string())
    }
}

/// The person `given`, or else that there is no such person.
fn person_id(given: i64) -> Result<PersonId, String> {
    id(given, str::parse, Missing::Person)
}

/// What People says of a change to person `id` refused for `refusal`.
fn said(id: PersonId, refusal: &ChangeRefusal) -> String {
    match refusal {
        ChangeRefusal::NoSuchPerson(_) => Missing::Person(id.to_string()).to_string(),
        ChangeRefusal::Protected(protected) => protected.to_string(),
        ChangeRefusal::Failed(e) => format!("{e:#}"),
    }
}

/// Change a person's role (`docs/specs/access.md`, Edit person).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EditPerson {
    person_id: i64,
    role: Role,
}

impl Tool for EditPerson {
    const ACTION: Action = Action::EditPerson;
    type Level = Manage;
    type Output = PersonOut;

    fn changing(&self) -> String {
        format!(
            "the role of person {}, to {}",
            self.person_id,
            self.role.name()
        )
    }

    async fn run(
        self,
        permit: &Permit<Manage>,
        context: &Context<'_>,
    ) -> Result<PersonOut, String> {
        let id = person_id(self.person_id)?;
        actions::edit_person(permit, context.sources, context.manager, id, self.role)
            .await
            .map(PersonOut::from)
            .map_err(|refusal| said(id, &refusal))
    }
}

/// Disable a person, signing them out and ending their authorizations
/// (`docs/specs/access.md`, Disable person).
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct DisablePerson {
    person_id: i64,
}

impl Tool for DisablePerson {
    const ACTION: Action = Action::DisablePerson;
    type Level = Manage;
    type Output = PersonOut;

    fn changing(&self) -> String {
        format!("person {}, disabled", self.person_id)
    }

    async fn run(
        self,
        permit: &Permit<Manage>,
        context: &Context<'_>,
    ) -> Result<PersonOut, String> {
        let id = person_id(self.person_id)?;
        actions::disable_person(permit, context.sources, context.manager, id)
            .await
            .map(PersonOut::from)
            .map_err(|refusal| said(id, &refusal))
    }
}

/// Enable a person (`docs/specs/access.md`, People, "Enable").
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct EnablePerson {
    person_id: i64,
}

impl Tool for EnablePerson {
    const ACTION: Action = Action::EnablePerson;
    type Level = Manage;
    type Output = PersonOut;

    fn changing(&self) -> String {
        format!("person {}, enabled", self.person_id)
    }

    async fn run(
        self,
        permit: &Permit<Manage>,
        context: &Context<'_>,
    ) -> Result<PersonOut, String> {
        let id = person_id(self.person_id)?;
        actions::enable_person(permit, context.sources, id)
            .await
            .map(PersonOut::from)
            .map_err(|refusal| said(id, &refusal))
    }
}
