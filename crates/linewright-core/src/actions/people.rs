//! The actions on people (`docs/specs/access.md`, People): seeing them,
//! changing their roles, and disabling and enabling them.

use anyhow::Result;

use super::{Action, Ended, Ending, logged};
use crate::access::people::{
    ChangeRefusal, ManagerName, People, Person, PersonId, Protected, may_change,
};
use crate::access::{Manage, Permit, Read, Role};
use crate::api::Sources;

/// The people of `serve`'s work directory.
fn people(sources: &Sources) -> People {
    People::new(sources.runs.database().clone())
}

/// A person as People lists them: with why they cannot lose the role of
/// manager or be disabled, if they cannot.
#[derive(Clone, Debug)]
pub struct PersonListed {
    pub person: Person,
    pub protected: Option<Protected>,
}

/// Everyone, in the order of their names, under the `manager` setting
/// `manager` (`docs/specs/access.md`, People).
pub async fn list_people(
    _: &Permit<Read>,
    sources: &Sources,
    manager: Option<&ManagerName>,
) -> Result<Vec<PersonListed>> {
    let everyone = people(sources).list().await?;
    Ok(everyone
        .iter()
        .map(|person| PersonListed {
            person: person.clone(),
            protected: may_change(person, &everyone, manager).err(),
        })
        .collect())
}

/// Gives person `id` the role `role` (`docs/specs/access.md`, Edit person).
///
/// # Errors
///
/// When there is no such person, the change would take the role of manager
/// from a person the `manager` setting `manager` names or from the last
/// manager, or it cannot be written.
pub async fn edit_person(
    permit: &Permit<Manage>,
    sources: &Sources,
    manager: Option<&ManagerName>,
    id: PersonId,
    role: Role,
) -> Result<Person, ChangeRefusal> {
    let changed = people(sources).set_role(id, role, manager).await;
    let how = Ended::of(&changed);
    logged(
        permit.holder(),
        Action::EditPerson,
        &format!(
            "the role of person {id}{}, to {}",
            identity_after(&changed),
            role.name()
        ),
        how,
    );
    changed
}

/// Disables person `id`, signing them out of every browser and ending
/// their authorizations (`docs/specs/access.md`, Disable person).
///
/// # Errors
///
/// When there is no such person, the `manager` setting `manager` names
/// them, they are the last manager, or it cannot be written.
pub async fn disable_person(
    permit: &Permit<Manage>,
    sources: &Sources,
    manager: Option<&ManagerName>,
    id: PersonId,
) -> Result<Person, ChangeRefusal> {
    let disabled = people(sources)
        .disable(id, manager, sources.clock.now())
        .await;
    let how = Ended::of(&disabled);
    logged(
        permit.holder(),
        Action::DisablePerson,
        &format!("person {id}{}, disabled", identity_after(&disabled)),
        how,
    );
    disabled
}

/// Enables person `id` (`docs/specs/access.md`, People, "Enable").
///
/// # Errors
///
/// When there is no such person, or it cannot be written.
pub async fn enable_person(
    permit: &Permit<Manage>,
    sources: &Sources,
    id: PersonId,
) -> Result<Person, ChangeRefusal> {
    let enabled = people(sources).enable(id).await;
    let how = Ended::of(&enabled);
    logged(
        permit.holder(),
        Action::EnablePerson,
        &format!("person {id}{}, enabled", identity_after(&enabled)),
        how,
    );
    enabled
}

/// What the log gives after a person's ID: the identity of the person a
/// change gave, in brackets, or nothing where it gave none.
fn identity_after(changed: &Result<Person, ChangeRefusal>) -> String {
    changed
        .as_ref()
        .map_or(String::new(), |person| format!(" ({})", person.logged()))
}

impl Ending for ChangeRefusal {
    fn failure(&self) -> Option<&anyhow::Error> {
        match self {
            ChangeRefusal::NoSuchPerson(_) | ChangeRefusal::Protected(_) => None,
            ChangeRefusal::Failed(e) => Some(e),
        }
    }
}
