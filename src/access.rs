//! Who is asking, and what they may do (`docs/specs/access.md`, Roles): the
//! access levels and roles, the caller a request resolves to, and the permit
//! that every action needs.

use std::fmt;
use std::marker::PhantomData;

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

/// A person's role (`docs/specs/access.md`, Roles).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Viewer,
    Contributor,
    Manager,
}

impl Role {
    /// The access level it gives.
    pub fn level(self) -> Level {
        match self {
            Role::Viewer => Level::Read,
            Role::Contributor => Level::Act,
            Role::Manager => Level::Manage,
        }
    }
}

/// Who a caller is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Who {
    /// Whoever reaches the web UI's port, while the UI signs no one in.
    Anyone,
}

impl fmt::Display for Who {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Who::Anyone => f.write_str("anyone"),
        }
    }
}

/// Who a request is from, with the role they have at the time of the
/// request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    who: Who,
    role: Role,
}

impl Caller {
    pub(crate) fn new(who: Who, role: Role) -> Caller {
        Caller { who, role }
    }

    /// Whoever reaches the web UI's port, who may do everything
    /// (`docs/specs/operation.md`, Serving the UI, Access), while the UI
    /// signs no one in.
    pub fn anyone() -> Caller {
        Caller::new(Who::Anyone, Role::Manager)
    }

    pub fn who(&self) -> &Who {
        &self.who
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// Whether its role reaches `level`, such as for whether a screen shows
    /// a button (`docs/specs/access.md`, Actions).
    pub fn can(&self, level: Level) -> bool {
        self.role.level() >= level
    }

    /// The permit of the level `L`, or else the refusal saying the level it
    /// needs, when its role does not reach it.
    pub fn permit<L: LevelMarker>(&self) -> Result<Permit<L>, NeedsLevel> {
        if self.can(L::LEVEL) {
            Ok(Permit {
                caller: self.clone(),
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

/// That its caller's role reached the access level `L` as the request was
/// resolved. Only [`Caller::permit`] makes one.
#[derive(Clone, Debug)]
pub struct Permit<L> {
    caller: Caller,
    level: PhantomData<L>,
}

impl<L> Permit<L> {
    /// Who it was given to.
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    fn lowered<M>(self) -> Permit<M> {
        Permit {
            caller: self.caller,
            level: PhantomData,
        }
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
}

impl From<Permit<Act>> for Permit<Read> {
    fn from(permit: Permit<Act>) -> Permit<Read> {
        permit.lowered()
    }
}

impl From<Permit<Manage>> for Permit<Act> {
    fn from(permit: Permit<Manage>) -> Permit<Act> {
        permit.lowered()
    }
}

impl From<Permit<Manage>> for Permit<Read> {
    fn from(permit: Permit<Manage>) -> Permit<Read> {
        permit.lowered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(role: Role) -> Caller {
        Caller::new(Who::Anyone, role)
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

    /// While the UI signs no one in, whoever reaches its port may do
    /// everything (`docs/specs/operation.md`, Serving the UI, Access).
    #[test_log::test]
    fn anyone_may_do_everything_while_the_ui_signs_no_one_in() {
        assert!(Caller::anyone().permit::<Manage>().is_ok());
    }
}
