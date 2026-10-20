use std::fmt::{self, Write as _};

use crate::domain::{Number, Timestamp};
use crate::phrase::{Phrase, Segment};
use crate::runs::{RunId, RunOf};

/// Markup with its text escaped, which a template may put in a page as it
/// is. Only this module makes one.
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

/// `phrase` as HTML (`docs/specs/design_system.md`): an item links to its
/// page and to GitHub, a time and a duration are `<time>` elements holding
/// the text the commands print, which the page's script shows in the
/// reader's time zone or locale and keeps up, and a label or a command is
/// code.
pub(crate) fn phrase(phrase: &Phrase, repo: &str) -> Html {
    let mut html = String::new();
    for segment in phrase.segments() {
        match segment {
            Segment::Text(text) => html.push_str(&escape(text).0),
            Segment::Item(number) => html.push_str(&item(*number, repo).0),
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
        page_link(number, &number.to_string()),
        github(number, repo),
    ))
}

/// #`number` and its title, linking to its page, and a link to it on
/// GitHub.
pub(crate) fn titled(number: Number, title: &str, repo: &str) -> Html {
    Html(format!(
        "{}{}",
        page_link(number, &format!("{number}: {title}")),
        github(number, repo),
    ))
}

/// `text` linking to the page of item `number`.
fn page_link(number: Number, text: &str) -> String {
    format!(
        r#"<a class="link link-hover" href="/items/{}">{}</a>"#,
        number.0,
        escape(text),
    )
}

fn github(number: Number, repo: &str) -> String {
    format!(
        r#"<a class="github" href="{href}" aria-label="{number} on GitHub" title="{number} on GitHub"></a>"#,
        href = escape(&format!("https://github.com/{repo}/issues/{}", number.0)),
    )
}

/// The run ID, its item linking to the item's page, and a link to the item
/// on GitHub.
pub(crate) fn run_id(id: &RunId, repo: &str) -> Html {
    let shown = id.to_string();
    match id.of {
        RunOf::Work { item, .. } => {
            let rest = shown.strip_prefix(&item.0.to_string()).unwrap_or_default();
            Html(format!(
                "{}{}{}",
                page_link(item, &item.0.to_string()),
                escape(rest),
                github(item, repo),
            ))
        }
        RunOf::Automation { .. } => escape(&shown),
    }
}

pub(crate) fn time(at: Timestamp) -> Html {
    Html(format!(
        r#"<time datetime="{}">{}</time>"#,
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
/// it only as time passes, which the page's script keeps up itself.
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
