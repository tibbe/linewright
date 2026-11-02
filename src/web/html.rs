use std::fmt::{self, Write as _};

use super::names::Names;
use crate::domain::{Number, Timestamp};
use crate::phrase::{Phrase, Segment};

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

/// `phrase` as HTML (`docs/specs/design_system.md`): an item is named as
/// `names` names it, a time and a duration are `<time>` elements holding
/// the text the commands print, and a label or a command is code.
pub(crate) fn phrase(phrase: &Phrase, names: &Names) -> Html {
    let mut html = String::new();
    for segment in phrase.segments() {
        match segment {
            Segment::Text(text) => html.push_str(&escape(text).0),
            Segment::Item(number) => html.push_str(&names.item(*number).0),
            Segment::Time(at) => html.push_str(&time(*at).0),
            Segment::Duration { seconds, since } => {
                let text = escape(&Phrase::default().duration(*seconds, None).to_string()).0;
                let _ = match since {
                    Some(since) => {
                        write!(html, r#"<time data-since="{}">{text}</time>"#, iso(*since))
                    }
                    None => write!(html, r#"<time datetime="PT{seconds}S">{text}</time>"#),
                };
            }
            Segment::Code(code) => {
                let _ = write!(html, "<code>{}</code>", escape(code));
            }
        }
    }
    Html(html)
}

/// #`number`, linking to its page, and a link to it on GitHub.
pub(crate) fn item(number: Number, repo: &str) -> Html {
    Html(format!(
        "{}{}",
        page_link(number, &escape(&number.to_string())),
        github(number, repo),
    ))
}

/// #`number` and its title, linking to its page, and a link to it on
/// GitHub. The number and the title are each a `span` of its own, of class
/// `number` and `title` (`docs/specs/design_system.md`, Layout).
pub(crate) fn titled(number: Number, title: &str, repo: &str) -> Html {
    Html(format!(
        "{}{}",
        page_link(
            number,
            &Html(format!(
                r#"<span class="number">{}</span> <span class="title">{}</span>"#,
                escape(&format!("{number}:")),
                escape(title),
            )),
        ),
        github(number, repo),
    ))
}

/// `content` in a `span` of `class`.
pub(crate) fn span(class: &str, content: &Html) -> Html {
    Html(format!(
        r#"<span class="{}">{content}</span>"#,
        escape(class)
    ))
}

/// `content` linking to the page of item `number`.
fn page_link(number: Number, content: &Html) -> String {
    format!(
        r#"<a class="link link-hover" href="/items/{}">{}</a>"#,
        number.0, content,
    )
}

fn github(number: Number, repo: &str) -> String {
    format!(
        r#"<a class="github" href="{href}" aria-label="{number} on GitHub" data-tip="{number} on GitHub"></a>"#,
        href = escape(&format!("https://github.com/{repo}/issues/{}", number.0)),
    )
}

/// `at` as a focusable `<time>` (`docs/specs/design_system.md`, Layout).
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
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(c),
        }
    }
    Html(escaped)
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
