use std::fmt::{self, Write as _};

use linewright_core::domain::Timestamp;
use linewright_core::phrase::{Phrase, Segment};

use super::names::Naming;

/// Markup with its text escaped, which a template may put in a page as it
/// is. Only this module makes one, [`rendered`] from a template or
/// [`built`] from a page's own builder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Html(String);

impl Html {
    #[must_use]
    pub(crate) fn then(mut self, html: &Html) -> Html {
        self.0.push_str(&html.0);
        self
    }
}

impl fmt::Display for Html {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl askama::filters::HtmlSafe for Html {}

/// `markup` that the pages' own builders made, escaping its text as they
/// went.
pub(crate) fn built(markup: String) -> Html {
    Html(markup)
}

/// `template` rendered, its text escaped as a template escapes it.
pub(crate) fn rendered(template: &impl askama::Template) -> Html {
    Html(super::render(template))
}

/// `phrase` as HTML (`docs/specs/board.md`, Item, Items in Waiting for and
/// in what keeps #N out, and `docs/specs/ui_patterns.md`, Time): an item is named as `naming` names it,
/// a time and a duration are `<time>` elements holding the text the commands
/// print, and a label or a command is code.
pub(crate) fn phrase(phrase: &Phrase, naming: &Naming) -> Html {
    let mut html = String::new();
    for segment in phrase.segments() {
        match segment {
            Segment::Text(text) => html.push_str(&escape(text).0),
            Segment::Item { number, kind } => html.push_str(&naming.item(*number, *kind).0),
            Segment::Time(at) => html.push_str(&time(*at).0),
            Segment::Duration { seconds, since } => {
                html.push_str(&duration(None, *seconds, *since).0);
            }
            Segment::Code(code) => {
                let _ = write!(html, "<code>{}</code>", escape(code));
            }
        }
    }
    Html(html)
}

/// `seconds` as a duration, or one running `since` a time and `seconds`
/// long when shown, as a `<time>`, of `class` if one is given, that the
/// page's script keeps up.
pub(crate) fn duration(class: Option<&str>, seconds: u64, since: Option<Timestamp>) -> Html {
    let text = escape(&Phrase::default().duration(seconds, None).to_string());
    let class = class.map_or(String::new(), |class| {
        format!(r#" class="{}""#, escape(class))
    });
    Html(match since {
        Some(since) => format!(r#"<time{class} data-since="{}">{text}</time>"#, iso(since)),
        None => format!(r#"<time{class} datetime="PT{seconds}S">{text}</time>"#),
    })
}

/// `content` in a `span` of `class`.
pub(crate) fn span(class: &str, content: &Html) -> Html {
    Html(format!(
        r#"<span class="{}">{content}</span>"#,
        escape(class)
    ))
}

/// `at` as a focusable `<time>` (`docs/specs/ui_patterns.md`, Layout).
pub(crate) fn time(at: Timestamp) -> Html {
    Html(format!(
        r#"<time datetime="{}" tabindex="0">{}</time>"#,
        iso(at),
        utc(at)
    ))
}

/// `at` as the commands print a time.
pub(crate) fn utc(at: Timestamp) -> Html {
    escape(&Phrase::default().time(at).to_string())
}

fn iso(at: Timestamp) -> String {
    at.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(crate) fn escape(text: &str) -> Html {
    Html(linewright_core::html::escaped(text))
}

/// `html` with the text of its running durations left out: what changes of
/// it only as time passes.
pub(crate) fn settled(html: &str) -> String {
    const RUNNING: &str = "<time data-since=\"";
    let mut settled = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(RUNNING) {
        let Some(open_end) = rest[start..].find('>').map(|end| start + end + 1) else {
            break;
        };
        settled.push_str(&rest[..open_end]);
        rest = &rest[open_end..];
        match rest.find("</time>") {
            Some(close) => rest = &rest[close..],
            None => break,
        }
    }
    settled.push_str(rest);
    settled
}
