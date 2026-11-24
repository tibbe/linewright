//! Who is asking, and what they may do (`docs/specs/access.md`, Roles): the
//! access levels and roles, the caller a request resolves to, and the permit
//! that every action needs.

pub mod oauth;
pub mod people;
pub mod providers;
pub mod sessions;
pub mod sign_ins;
pub(crate) mod tokens;

use std::fmt;
use std::marker::PhantomData;

use people::{PersonId, State};

/// An access level (`docs/specs/access.md`, Roles), each including the
/// ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Read,
    Act,
    Manage,
}

impl Level {
    /// Its name, as the spec gives it.
    pub fn name(self) -> &'static str {
        match self {
            Level::Read => "read",
            Level::Act => "act",
            Level::Manage => "manage",
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A person's role (`docs/specs/access.md`, Roles), by its name.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[schemars(inline)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Contributor,
    Manager,
}

impl Role {
    /// Every role, in the order of the spec's table.
    pub const ALL: [Role; 3] = [Role::Viewer, Role::Contributor, Role::Manager];

    /// Its name, as the spec gives it.
    pub fn name(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Contributor => "contributor",
            Role::Manager => "manager",
        }
    }

    /// The role named `name`, as [`Role::name`] gives it.
    pub fn named(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|role| role.name() == name)
    }

    /// The access level it gives.
    pub fn level(self) -> Level {
        match self {
            Role::Viewer => Level::Read,
            Role::Contributor => Level::Act,
            Role::Manager => Level::Manage,
        }
    }
}

/// Who a request is from: an enabled person, with the role they have at
/// the time of the request (`docs/specs/access.md`, Roles, At each request).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    person: people::Person,
}

impl Caller {
    /// `person` asking, `None` while they are disabled.
    pub fn of(person: people::Person) -> Option<Caller> {
        (person.state == State::Enabled).then_some(Caller { person })
    }

    pub fn person(&self) -> &people::Person {
        &self.person
    }

    pub fn role(&self) -> Role {
        self.person.role
    }

    /// Whether its role reaches `level`, such as for whether a screen shows
    /// a button (`docs/specs/access.md`, Actions).
    pub fn can(&self, level: Level) -> bool {
        self.role().level() >= level
    }

    /// The permit of the level `L`, or else the refusal saying the level it
    /// needs, when its role does not reach it.
    pub fn permit<L: LevelMarker>(&self) -> Result<Permit<L>, NeedsLevel> {
        if self.can(L::LEVEL) {
            Ok(Permit {
                holder: Person(self.clone()),
                level: PhantomData,
            })
        } else {
            Err(NeedsLevel(L::LEVEL))
        }
    }
}

/// The refusal of an action whose access level the caller's role does not
/// reach (`docs/specs/access.md`, Actions, An access level that is too low).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("This needs the {0} access level.")]
pub struct NeedsLevel(pub Level);

/// That its holder reached the access level `L` as the request was
/// resolved: a person, whose permit only [`Caller::permit`] makes, or the
/// operator, whose permit only this crate makes.
#[derive(Clone, Debug)]
pub struct Permit<L, H = Person> {
    holder: H,
    level: PhantomData<L>,
}

impl<L, H: Holder> Permit<L, H> {
    pub(crate) fn holder(&self) -> &H {
        &self.holder
    }

    fn lowered<M>(self) -> Permit<M, H> {
        Permit {
            holder: self.holder,
            level: PhantomData,
        }
    }
}

impl<L> Permit<L, Person> {
    /// Who it was given to.
    pub fn caller(&self) -> &Caller {
        &self.holder.0
    }
}

impl<L: LevelMarker> Permit<L, Operator> {
    /// The operator's permit, which is of every level
    /// (`docs/specs/access.md`, Out of scope: who can open the socket can use
    /// the commands).
    pub(crate) fn operator() -> Permit<L, Operator> {
        Permit {
            holder: Operator,
            level: PhantomData,
        }
    }
}

/// Who holds a [`Permit`].
pub trait Holder: sealed::SealedHolder {
    /// Who they are in the log (`docs/specs/access.md`, Actions, Logged).
    fn logged(&self) -> Logged;
}

/// Who holds a permit, as the log gives them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Logged {
    Person { id: PersonId, identity: String },
    Operator,
}

/// A person, as the caller of a request.
#[derive(Clone, Debug)]
pub struct Person(Caller);

impl Person {
    pub(crate) fn of(caller: Caller) -> Person {
        Person(caller)
    }
}

/// The operator, asking through the local API.
#[derive(Clone, Copy, Debug)]
pub struct Operator;

impl Holder for Person {
    fn logged(&self) -> Logged {
        let person = self.0.person();
        Logged::Person {
            id: person.id,
            identity: person.logged(),
        }
    }
}

impl Holder for Operator {
    fn logged(&self) -> Logged {
        Logged::Operator
    }
}

/// The type of an access level, which a [`Permit`] is of.
pub trait LevelMarker: sealed::Sealed {
    const LEVEL: Level;
}

/// The read access level's type.
#[derive(Clone, Copy, Debug)]
pub struct Read;

/// The act access level's type.
#[derive(Clone, Copy, Debug)]
pub struct Act;

/// The manage access level's type.
#[derive(Clone, Copy, Debug)]
pub struct Manage;

impl LevelMarker for Read {
    const LEVEL: Level = Level::Read;
}

impl LevelMarker for Act {
    const LEVEL: Level = Level::Act;
}

impl LevelMarker for Manage {
    const LEVEL: Level = Level::Manage;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Read {}
    impl Sealed for super::Act {}
    impl Sealed for super::Manage {}

    pub trait SealedHolder {}
    impl SealedHolder for super::Person {}
    impl SealedHolder for super::Operator {}
}

impl<H: Holder> From<Permit<Act, H>> for Permit<Read, H> {
    fn from(permit: Permit<Act, H>) -> Permit<Read, H> {
        permit.lowered()
    }
}

impl<H: Holder> From<Permit<Manage, H>> for Permit<Act, H> {
    fn from(permit: Permit<Manage, H>) -> Permit<Act, H> {
        permit.lowered()
    }
}

impl<H: Holder> From<Permit<Manage, H>> for Permit<Read, H> {
    fn from(permit: Permit<Manage, H>) -> Permit<Read, H> {
        permit.lowered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(role: Role) -> Caller {
        Caller::of(people::tests::person(1, role, State::Enabled)).unwrap()
    }

    /// `docs/specs/access.md`, Roles: each level includes the ones above it
    /// in the table.
    #[test_log::test]
    fn a_role_permits_its_own_level_and_the_ones_above_it_in_the_table() {
        let viewer = caller(Role::Viewer);
        let contributor = caller(Role::Contributor);
        let manager = caller(Role::Manager);

        assert!(viewer.permit::<Read>().is_ok());
        assert_eq!(viewer.permit::<Act>().unwrap_err(), NeedsLevel(Level::Act));
        assert_eq!(
            viewer.permit::<Manage>().unwrap_err(),
            NeedsLevel(Level::Manage)
        );
        assert!(contributor.permit::<Read>().is_ok());
        assert!(contributor.permit::<Act>().is_ok());
        assert_eq!(
            contributor.permit::<Manage>().unwrap_err(),
            NeedsLevel(Level::Manage)
        );
        assert!(manager.permit::<Read>().is_ok());
        assert!(manager.permit::<Act>().is_ok());
        assert!(manager.permit::<Manage>().is_ok());

        assert_eq!(
            [Level::Read, Level::Act, Level::Manage].map(|level| contributor.can(level)),
            [true, true, false]
        );
    }

    /// `docs/specs/access.md`, Actions, An access level that is too low.
    #[test_log::test]
    fn a_refusal_names_the_level_it_needs() {
        let refused = caller(Role::Viewer).permit::<Act>().unwrap_err();

        assert_eq!(refused.to_string(), "This needs the act access level.");
        assert_eq!(
            NeedsLevel(Level::Manage).to_string(),
            "This needs the manage access level."
        );
    }

    #[test_log::test]
    fn a_higher_permit_is_a_lower_one_of_the_same_caller() {
        let manager = caller(Role::Manager);
        let managing = manager.permit::<Manage>().unwrap();

        let reading: Permit<Read> = Permit::<Act>::from(managing.clone()).into();
        let read_directly: Permit<Read> = managing.into();

        assert_eq!(reading.caller(), &manager);
        assert_eq!(read_directly.caller(), &manager);
    }

    /// `docs/specs/access.md`, People, State: a disabled person asks
    /// nothing.
    #[test_log::test]
    fn a_disabled_person_is_no_caller() {
        let disabled = people::tests::person(1, Role::Manager, State::Disabled);

        assert_eq!(Caller::of(disabled), None);
    }
}
