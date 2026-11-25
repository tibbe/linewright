//! People (`docs/specs/access.md`, People): a row per person, and for a
//! manager, "Edit", which opens Edit person, "Disable", which asks first in
//! Disable person, and "Enable".

use askama::Template;
use askama_web::WebTemplate;
use axum::Extension;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use linewright_core::access::people::{
    ChangeRefusal, ManagerName, Person, PersonId, State as PersonState,
};
use linewright_core::access::{Manage, Permit, Read, Role};
use linewright_core::actions::{self, Action, PersonListed};
use linewright_core::api::Sources;
use linewright_core::phrase::Status;

use super::buttons::Verb;
use super::caller::{Asking, Reader, Watch};
use super::confirmation::{Confirmation, TurnDown};
use super::editing::{EditDialog, edit_values, gone, read_sent};
use super::html::{self, Html};
use super::logos::identity_logo;
use super::making::{Answer, Field, FieldRefusals, Form, Making, REQUIRED, Refusal};
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Area, Frame, Missing, Navigation, Place};
use super::table::Align;
use super::write::{Refused, Write};
use super::{ErrorPage, Page, events, render, with_filters};

/// The path of People (`docs/specs/access.md`, People, Address).
const PATH: &str = "/people";

/// The `manager` setting, as each request of People has it.
#[derive(Clone, Debug)]
pub(super) struct Named(pub(super) Option<ManagerName>);

/// The id of where People says why a change it asked first for, or
/// "Enable", was refused.
const SAID: &str = "people-said";

/// The region of People's stream that holds the [`edit_values`] of each
/// Edit person dialog.
const EDIT_PEOPLE_VALUES: &str = "edit-people-values";

/// A column of the table of people, the first naming each row.
#[derive(Clone, Copy)]
enum PeopleColumn {
    Person,
    Role,
    State,
    SignedIn,
    /// A row's buttons, with an empty head, while the reader's role allows
    /// them.
    Buttons,
}

impl PeopleColumn {
    fn head(self) -> &'static str {
        match self {
            PeopleColumn::Person => "Person",
            PeopleColumn::Role => "Role",
            PeopleColumn::State => "State",
            PeopleColumn::SignedIn => "Signed in",
            PeopleColumn::Buttons => "",
        }
    }

    /// How the column's head and cells align.
    fn align(self) -> Align {
        match self {
            PeopleColumn::Person
            | PeopleColumn::Role
            | PeopleColumn::State
            | PeopleColumn::SignedIn
            | PeopleColumn::Buttons => Align::Start,
        }
    }
}

/// The people, ready for the page: a page of their rows, with the pages
/// under them.
struct PeopleShown {
    columns: Vec<PeopleColumn>,
    rows: Vec<RowShown>,
    pages: Option<PagesShown>,
}

struct RowShown {
    /// What they are shown by: their name, or else their identity.
    name: String,
    avatar: Option<String>,
    /// What stands in for the avatar without one.
    letter: String,
    logo: Html,
    identity: String,
    role: &'static str,
    status: Status,
    state: &'static str,
    signed_in: Html,
    /// `None` while the reader's role does not allow them.
    buttons: Option<RowButtons>,
}

/// A row's buttons (`docs/specs/access.md`, People, A row's buttons).
struct RowButtons {
    /// Why "Edit" and "Disable" are disabled, shown on hover, if they are.
    protected: Option<String>,
    edit: String,
    state: StateButton,
}

/// The button after "Edit", which changes a person's state.
enum StateButton {
    /// "Disable" on an enabled person, opening the confirmation of this id.
    Disable(String),
    /// "Enable" on a disabled person, posting to this path.
    Enable(String),
}

/// How a state shows (`docs/specs/access.md`, People (the screen), State).
fn state_shown(state: PersonState) -> (Status, &'static str) {
    match state {
        PersonState::Enabled => (Status::Done, "enabled"),
        PersonState::Disabled => (Status::Cancelled, "disabled"),
    }
}

/// The path of person `id`'s `change`, such as `edit`.
fn person_path(id: PersonId, change: &str) -> String {
    format!("{PATH}/{id}/{change}")
}

/// The id of person `id`'s Edit person dialog.
fn edit_dialog(id: PersonId) -> String {
    format!("edit-person-{id}")
}

/// The id of person `id`'s Disable person confirmation.
fn disable_dialog(id: PersonId) -> String {
    format!("disable-person-{id}")
}

impl PeopleShown {
    fn of(people: &[PersonListed], manages: bool, requested: Requested) -> PeopleShown {
        let pages = Pages::of(people.len(), requested);
        let mut columns = vec![
            PeopleColumn::Person,
            PeopleColumn::Role,
            PeopleColumn::State,
            PeopleColumn::SignedIn,
        ];
        if manages {
            columns.push(PeopleColumn::Buttons);
        }
        PeopleShown {
            columns,
            rows: people[pages.range()]
                .iter()
                .map(|PersonListed { person, protected }| {
                    let (status, state) = state_shown(person.state);
                    RowShown {
                        name: person.shown_name().to_string(),
                        avatar: person.avatar.as_ref().map(ToString::to_string),
                        letter: person.letter(),
                        logo: identity_logo(person.identity.provider),
                        identity: person.shown_as.clone(),
                        role: person.role.name(),
                        status,
                        state,
                        signed_in: html::time(person.signed_in_at),
                        buttons: manages.then(|| RowButtons {
                            protected: protected.as_ref().map(ToString::to_string),
                            edit: edit_dialog(person.id),
                            state: match person.state {
                                PersonState::Enabled => {
                                    StateButton::Disable(disable_dialog(person.id))
                                }
                                PersonState::Disabled => {
                                    StateButton::Enable(person_path(person.id, "enable"))
                                }
                            },
                        }),
                    }
                })
                .collect(),
            pages: pages.shown("people", |page| PageHref::at(path(PATH, page))),
        }
    }
}

/// The fields of Edit person, as it sends them.
#[derive(Clone, Default, Deserialize)]
pub(super) struct PersonFields {
    #[serde(default)]
    role: String,
}

impl Form for PersonFields {
    type Field = PersonField;
}

/// The field of Edit person.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum PersonField {
    Role,
}

impl Field for PersonField {
    fn required(&self) -> bool {
        match self {
            PersonField::Role => true,
        }
    }
}

/// What Edit person's form holds: the dialog `id`'s.
#[derive(Template)]
#[template(path = "edit_person.html")]
pub(super) struct EditPersonForm {
    id: String,
    making: Making<PersonFields>,
}

impl EditPersonForm {
    /// The id of its field.
    fn field_id(&self) -> String {
        format!("{}-role", self.id)
    }

    /// Whether the field holds `role`.
    #[expect(
        clippy::trivially_copy_pass_by_ref,
        reason = "askama passes a template's values by reference"
    )]
    fn chosen(&self, role: &Role) -> bool {
        self.making.fields.role == role.name()
    }
}

/// Edit person of `person` (`docs/specs/access.md`, Edit person), holding
/// their role.
fn edit_person(person: &Person) -> EditDialog {
    let id = edit_dialog(person.id);
    EditDialog {
        action: person_path(person.id, "edit"),
        form: html::rendered(&EditPersonForm {
            id: id.clone(),
            making: Making::of(PersonFields {
                role: person.role.name().into(),
            }),
        }),
        id,
    }
}

/// Disable person of `person` (`docs/specs/access.md`, Disable person).
fn disable_person(person: &Person) -> Confirmation {
    Confirmation {
        id: disable_dialog(person.id),
        verb: Verb::Disable,
        question: format!(
            "Disable {}? This signs them out and ends their authorizations.",
            person.shown_name()
        ),
        turn_down: TurnDown::Cancel,
        action: person_path(person.id, "disable"),
        fields: vec![],
        said: SAID,
    }
}

/// The values each Edit person dialog starts from, the person's role as it
/// is now, in the region [`EDIT_PEOPLE_VALUES`]: none while the reader's
/// role does not allow Edit person.
fn people_values(people: &[PersonListed], manages: bool) -> String {
    let values: String = people
        .iter()
        .filter(|_| manages)
        .map(|listed| {
            edit_values(
                &edit_dialog(listed.person.id),
                vec![("role", listed.person.role.name().into())],
            )
        })
        .collect();
    format!(r#"<div id="{EDIT_PEOPLE_VALUES}" hidden>{values}</div>"#)
}

/// What People shows of everyone, as the reader's role allows.
struct Shown {
    people: PeopleShown,
    /// Edit person and Disable person of each person, while the reader's
    /// role allows them: a region of the stream, so that a row People gains
    /// has its own.
    dialogs: Dialogs,
    values: String,
}

impl Shown {
    fn of(people: &[PersonListed], frame: &Frame, requested: Requested) -> Shown {
        let manages = frame.allows(Action::EditPerson);
        let managed = || {
            people
                .iter()
                .filter(|_| manages)
                .map(|listed| &listed.person)
        };
        Shown {
            people: PeopleShown::of(people, manages, requested),
            dialogs: Dialogs {
                edits: managed().map(edit_person).collect(),
                disables: managed().map(disable_person).collect(),
            },
            values: people_values(people, manages),
        }
    }

    /// Its regions of People's stream.
    fn regions(&self) -> [(&'static str, String); 3] {
        [
            (
                "people",
                render(&PeopleRegion {
                    people: &self.people,
                }),
            ),
            (DIALOGS, render(&self.dialogs)),
            (EDIT_PEOPLE_VALUES, self.values.clone()),
        ]
    }
}

/// The region of People's stream that holds Edit person and Disable person
/// of each person.
const DIALOGS: &str = "people-dialogs";

#[derive(Template)]
#[template(path = "people_dialogs.html")]
struct Dialogs {
    edits: Vec<EditDialog>,
    disables: Vec<Confirmation>,
}

#[derive(Template, WebTemplate)]
#[template(path = "people.html")]
struct PeoplePage {
    page: Page,
    frame: Frame,
    people: PeopleShown,
    /// The [`Dialogs`], rendered.
    dialogs: Html,
    values: String,
}

#[derive(Template)]
#[template(path = "people.html", block = "people")]
struct PeopleRegion<'a> {
    people: &'a PeopleShown,
}

/// What the page's address carries: the page of the table.
#[derive(Deserialize)]
pub(super) struct PeopleQuery {
    #[serde(default)]
    page: Requested,
}

/// `path` with the page `page` as its query.
fn path(path: &str, page: Requested) -> String {
    with_filters(path, &[("page", &page.field())])
}

/// The page, its stream's on the `requested` page.
fn page(requested: Requested, navigation: Navigation) -> Page {
    Page::new(
        Place::Navigation(Area::People),
        path("/events/people", requested),
        navigation,
    )
    .filling_the_viewport()
}

/// The frame and everyone, as `permit`'s reader sees them under the
/// `manager` setting `named`.
async fn read(
    permit: &Permit<Read>,
    sources: &Sources,
    named: &Named,
) -> (Frame, anyhow::Result<Vec<PersonListed>>) {
    let frame = Frame::of(permit, sources).await;
    let people = actions::list_people(permit, sources, named.0.as_ref()).await;
    (frame, people)
}

pub(super) async fn people_page(
    Reader { permit, sources }: Reader,
    Query(query): Query<PeopleQuery>,
    navigation: Navigation,
    Extension(named): Extension<Named>,
) -> Response {
    let requested = query.page;
    let page = page(requested, navigation);
    let (frame, people) = read(&permit, &sources, &named).await;
    match people {
        Ok(people) => {
            let Shown {
                people,
                dialogs,
                values,
            } = Shown::of(&people, &frame, requested);
            PeoplePage {
                page,
                frame,
                people,
                dialogs: html::rendered(&dialogs),
                values,
            }
            .into_response()
        }
        Err(e) => {
            let page = ErrorPage {
                page,
                frame,
                region: "people",
                error: format!("{e:#}"),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

/// People's regions: the shell's, the people, their dialogs and the values
/// Edit person starts from, each as the reader's role allows at the time
/// (`docs/specs/access.md`, Roles, At each request).
pub(super) async fn people_events(
    State(sources): State<Sources>,
    watch: Watch,
    Query(query): Query<PeopleQuery>,
    navigation: Navigation,
    Extension(named): Extension<Named>,
) -> Response {
    let requested = query.page;
    let reader = watch.clone();
    let regions = move || {
        let (reader, sources, named) = (reader.clone(), sources.clone(), named.clone());
        async move {
            let Some(permit) = reader
                .caller()
                .await
                .and_then(|caller| caller.permit().ok())
            else {
                return vec![];
            };
            let (frame, people) = read(&permit, &sources, &named).await;
            let mut regions = shell::regions(&frame, &page(requested, navigation));
            if let Ok(people) = people {
                regions.extend(Shown::of(&people, &frame, requested).regions());
            }
            regions
        }
    };
    events(regions, watch)
}

/// Gives person `id`, as the path gives it, the role the form in `body`
/// sends, and answers with Edit person's form: holding the role saved, or
/// as sent with why it was refused (`docs/specs/access.md`, Edit person,
/// Refusals). Refused, writing nothing, unless the browser shows the request
/// came from the web UI's own pages.
pub(super) async fn save_person(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    Extension(named): Extension<Named>,
    uri: Uri,
    body: Bytes,
) -> Answer<EditPersonForm> {
    let (fields, sent) =
        read_sent::<Manage, PersonFields>(Action::EditPerson, &asking, &uri, &body);
    let parsed = id.parse::<PersonId>();
    let dialog = parsed.as_ref().map_or_else(
        |_| format!("edit-person-{id}"),
        |person| edit_dialog(*person),
    );
    let form = |making| EditPersonForm {
        id: dialog.clone(),
        making,
    };
    let refuse = |(status, refusal)| Answer::Refused {
        status,
        form: form(Making::refused(fields, refusal)),
    };
    let (permit, sent) = match sent {
        Ok(permitted) => permitted,
        Err((status, why)) => return refuse((status, Refusal::Whole(why))),
    };
    let Ok(person) = parsed else {
        return refuse(gone(&Missing::Person(id)));
    };
    let Some(role) = Role::named(&sent.role) else {
        return refuse((
            StatusCode::UNPROCESSABLE_ENTITY,
            Refusal::Fields(FieldRefusals::of(PersonField::Role, REQUIRED)),
        ));
    };
    match actions::edit_person(&permit, &sources, named.0.as_ref(), person, role).await {
        Ok(person) => Answer::Saved {
            form: form(Making::of(PersonFields {
                role: person.role.name().into(),
            })),
        },
        Err(refusal) => refuse({
            let (status, why) = refused(&id, &refusal);
            (status, Refusal::Whole(why))
        }),
    }
}

/// Disables person `id`, as the path gives it (`docs/specs/access.md`,
/// Disable person): 200 with nothing to say, as the table shows it, or else
/// why not. Refused, writing nothing, unless the browser shows the request
/// came from the web UI's own pages.
pub(super) async fn disable(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    Extension(named): Extension<Named>,
    uri: Uri,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Disable;
    let permit = WRITE.permit::<Manage>(Action::DisablePerson, &asking, &uri)?;
    let person = parse(WRITE, &id)?;
    let disabled = actions::disable_person(&permit, &sources, named.0.as_ref(), person).await;
    answer(WRITE, &id, disabled)
}

/// Enables person `id`, as the path gives it (`docs/specs/access.md`,
/// People, "Enable"): 200 with nothing to say, as the table shows it, or
/// else why not. Refused, writing nothing, unless the browser shows the
/// request came from the web UI's own pages.
pub(super) async fn enable(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::Enable;
    let permit = WRITE.permit::<Manage>(Action::EnablePerson, &asking, &uri)?;
    let person = parse(WRITE, &id)?;
    let enabled = actions::enable_person(&permit, &sources, person).await;
    answer(WRITE, &id, enabled)
}

/// The person `id`, as a path gives it, or else `write` refused as of a
/// person who does not exist.
fn parse(write: Write, id: &str) -> Result<PersonId, Refused> {
    id.parse().map_err(|_| {
        let (status, why) = no_such(id);
        write.refused(status, &why)
    })
}

/// The status a change to person `id`, as a path gives it, who does not
/// exist, is answered with, and what People says of it.
fn no_such(id: &str) -> (StatusCode, String) {
    (
        StatusCode::NOT_FOUND,
        Missing::Person(id.into()).to_string(),
    )
}

/// The status a change to person `id`, as a path gives it, is answered with
/// for `refusal`, and what People says of it.
fn refused(id: &str, refusal: &ChangeRefusal) -> (StatusCode, String) {
    match refusal {
        ChangeRefusal::NoSuchPerson(_) => no_such(id),
        ChangeRefusal::Protected(protected) => {
            (StatusCode::UNPROCESSABLE_ENTITY, protected.to_string())
        }
        ChangeRefusal::Failed(e) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
    }
}

/// What People says of `write` to person `id`, as a path gives it, that
/// ended in `changed`: nothing once done, as the table shows it.
fn answer(
    write: Write,
    id: &str,
    changed: Result<Person, ChangeRefusal>,
) -> Result<Response, Refused> {
    match changed {
        Ok(_) => Ok((StatusCode::OK, axum::response::Html("")).into_response()),
        Err(refusal) => {
            let (status, why) = refused(id, &refusal);
            Err(write.refused(status, &why))
        }
    }
}
