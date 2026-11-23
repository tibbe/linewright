//! Authorizations (`docs/specs/access.md`, Authorizations and End
//! authorization; `docs/specs/web/access.md`, Authorizations): a row per
//! authorization of the reader, newest first, each with "End", which asks
//! first in End authorization.

use askama::Template;
use askama_web::WebTemplate;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use super::buttons::Verb;
use super::caller::{Asking, Reader, Watch};
use super::confirmation::{Confirmation, TurnDown};
use super::html::{self, Html};
use super::icons::CardIcon;
use super::pages::{PageHref, Pages, PagesShown, Requested};
use super::shell::{self, Frame, Navigation, Place};
use super::write::{Refused, Write};
use super::{ErrorPage, Page, events, render, with_filters};
use crate::access::oauth::store::{AuthorizationListed, EndRefusal};
use crate::access::oauth::{AuthorizationId, client_name};
use crate::access::{Permit, Read};
use crate::actions::{self, Action};
use crate::api::Sources;

/// The path of Authorizations (`docs/specs/web/design_system.md`,
/// Navigation).
pub(crate) const PATH: &str = "/authorizations";

/// The id of where Authorizations says why an end was refused.
const SAID: &str = "authorizations-said";

/// The region of Authorizations' stream that holds End authorization of
/// each authorization.
const DIALOGS: &str = "authorizations-dialogs";

/// The heads of the table's columns, the last over the rows' buttons.
const HEADS: [&str; 5] = ["Client", "Sends back to", "Allowed", "Ends", ""];

/// The authorizations, ready for the page: a page of their rows, with the
/// pages under them.
struct AuthorizationsShown {
    rows: Vec<RowShown>,
    pages: Option<PagesShown>,
}

struct RowShown {
    client: String,
    sends_back_to: String,
    allowed: Html,
    ends: Html,
    /// The id of its End authorization.
    end: String,
}

/// The id of authorization `id`'s End authorization.
fn end_dialog(id: AuthorizationId) -> String {
    format!("end-authorization-{id}")
}

/// The path that ends authorization `id`.
fn end_path(id: AuthorizationId) -> String {
    format!("{PATH}/{id}/end")
}

impl AuthorizationsShown {
    fn of(authorizations: &[AuthorizationListed], requested: Requested) -> AuthorizationsShown {
        let pages = Pages::of(authorizations.len(), requested);
        AuthorizationsShown {
            rows: authorizations[pages.range()]
                .iter()
                .map(|authorization| RowShown {
                    client: client_name(authorization.client_name.as_deref()).to_string(),
                    sends_back_to: authorization.redirect_uri.host().to_string(),
                    allowed: html::time(authorization.allowed_at),
                    ends: html::time(authorization.ends_at),
                    end: end_dialog(authorization.id),
                })
                .collect(),
            pages: pages.shown("authorizations", |page| PageHref::at(path(page))),
        }
    }
}

/// End authorization of `authorization` (`docs/specs/access.md`, End
/// authorization).
fn end_authorization(authorization: &AuthorizationListed) -> Confirmation {
    let name = client_name(authorization.client_name.as_deref());
    let mut first = name.chars();
    let opening = first
        .next()
        .map(|letter| letter.to_uppercase().chain(first).collect::<String>())
        .unwrap_or_default();
    Confirmation {
        id: end_dialog(authorization.id),
        verb: Verb::End,
        question: format!("End the authorization of {name}? {opening} can no longer act as you."),
        turn_down: TurnDown::Cancel,
        action: end_path(authorization.id),
        fields: vec![],
        said: SAID,
    }
}

#[derive(Template)]
#[template(path = "authorizations.html", block = "dialogs")]
struct Dialogs {
    ends: Vec<Confirmation>,
}

#[derive(Template, WebTemplate)]
#[template(path = "authorizations.html")]
struct AuthorizationsPage {
    page: Page,
    frame: Frame,
    authorizations: AuthorizationsShown,
    ends: Vec<Confirmation>,
}

#[derive(Template)]
#[template(path = "authorizations.html", block = "authorizations")]
struct AuthorizationsRegion<'a> {
    authorizations: &'a AuthorizationsShown,
}

/// What the page's address carries: the page of the table.
#[derive(Deserialize)]
pub(super) struct AuthorizationsQuery {
    #[serde(default)]
    page: Requested,
}

/// [`PATH`] with the page `page` as its query.
fn path(page: Requested) -> String {
    with_filters(PATH, &[("page", &page.field())])
}

/// The page, its stream's on the `requested` page.
fn page(requested: Requested, navigation: Navigation) -> Page {
    Page::new(
        Place::Account {
            name: "Authorizations",
            icon: CardIcon::Authorizations,
        },
        with_filters("/events/authorizations", &[("page", &requested.field())]),
        navigation,
    )
    .filling_the_viewport()
}

/// The frame and the reader's authorizations.
async fn read(
    permit: &Permit<Read>,
    sources: &Sources,
) -> (Frame, anyhow::Result<Vec<AuthorizationListed>>) {
    let frame = Frame::of(permit, sources).await;
    let authorizations = actions::list_authorizations(permit, sources).await;
    (frame, authorizations)
}

pub(super) async fn authorizations_page(
    Reader { permit, sources }: Reader,
    Query(query): Query<AuthorizationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let page = page(requested, navigation);
    let (frame, authorizations) = read(&permit, &sources).await;
    match authorizations {
        Ok(authorizations) => AuthorizationsPage {
            page,
            frame,
            ends: authorizations.iter().map(end_authorization).collect(),
            authorizations: AuthorizationsShown::of(&authorizations, requested),
        }
        .into_response(),
        Err(e) => {
            let page = ErrorPage {
                page,
                frame,
                region: "authorizations",
                error: format!("{e:#}"),
            };
            (StatusCode::INTERNAL_SERVER_ERROR, page).into_response()
        }
    }
}

/// Authorizations' regions: the shell's, the authorizations and their End
/// authorization, as the reader's at the time.
pub(super) async fn authorizations_events(
    State(sources): State<Sources>,
    watch: Watch,
    Query(query): Query<AuthorizationsQuery>,
    navigation: Navigation,
) -> Response {
    let requested = query.page;
    let reader = watch.clone();
    let regions = move || {
        let (reader, sources) = (reader.clone(), sources.clone());
        async move {
            let Some(permit) = reader
                .caller()
                .await
                .and_then(|caller| caller.permit().ok())
            else {
                return vec![];
            };
            let (frame, authorizations) = read(&permit, &sources).await;
            let mut regions = shell::regions(&frame, &page(requested, navigation));
            if let Ok(authorizations) = authorizations {
                regions.push((
                    "authorizations",
                    render(&AuthorizationsRegion {
                        authorizations: &AuthorizationsShown::of(&authorizations, requested),
                    }),
                ));
                regions.push((
                    DIALOGS,
                    render(&Dialogs {
                        ends: authorizations.iter().map(end_authorization).collect(),
                    }),
                ));
            }
            regions
        }
    };
    events(regions, watch)
}

/// Ends the reader's authorization `id`, as the path gives it
/// (`docs/specs/access.md`, End authorization): 200 with nothing to say,
/// as the table shows it, or else why not. Refused, writing nothing, unless
/// the browser shows the request came from the web UI's own pages.
pub(super) async fn end(
    State(sources): State<Sources>,
    asking: Asking,
    Path(id): Path<String>,
    uri: Uri,
) -> Result<Response, Refused> {
    const WRITE: Write = Write::End;
    let permit = WRITE.permit::<Read>(Action::EndAuthorization, &asking, &uri)?;
    let no_such = || {
        WRITE.refused(
            StatusCode::NOT_FOUND,
            &format!("No such authorization {id}"),
        )
    };
    let authorization = id.parse::<AuthorizationId>().map_err(|_| no_such())?;
    match actions::end_authorization(&permit, &sources, authorization).await {
        Ok(_) => Ok((StatusCode::OK, axum::response::Html("")).into_response()),
        Err(EndRefusal::NoSuchAuthorization(_)) => Err(no_such()),
        Err(EndRefusal::Failed(e)) => {
            Err(WRITE.refused(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")))
        }
    }
}
