use std::fmt;

use chrono::TimeDelta;

use crate::domain::{ItemKind, Number, Timestamp};

/// Wording the monitoring commands show, with the items, times, durations
/// and labels it names kept apart from its text, so that each can be shown as
/// suits where it is shown. `Display` gives it as the commands print it.
#[derive(Clone, Debug, Default, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct Phrase(Vec<Segment>);

/// What an outcome or a state shows as (`docs/specs/design_system.md`, Status),
/// each with its own icon and colour
/// (`docs/specs/design_system.md`, Status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Done,
    Failed,
    Waiting,
    InFlight,
    Paused,
    Cancelled,
}

impl Status {
    /// Its badge, as the classes of the spec's table
    /// (`docs/specs/components.md`, Status badge): a badge gives each daisyUI
    /// colour its own colour of text, which reads on it in either theme
    /// (<https://daisyui.com/docs/colors/>), while paused, whose `base-content/60` is a text colour with no
    /// content colour of its own, is a paused badge
    /// (`docs/specs/components.md`, Status badge), an outline in that colour
    /// with no fill. Written whole, since Tailwind finds the classes the
    /// stylesheet needs by scanning the sources' text
    /// (<https://tailwindcss.com/docs/detecting-classes-in-source-files>).
    pub fn badge(self) -> &'static str {
        match self {
            Status::Done => "badge badge-success",
            Status::Failed => "badge badge-error",
            Status::InFlight => "badge badge-info",
            Status::Waiting => "badge badge-warning",
            Status::Cancelled => "badge badge-neutral",
            Status::Paused => "badge badge-outline text-base-content/60",
        }
    }

    /// The classes of a daisyUI `status` dot in its colour, for a status
    /// among facts (`docs/specs/components.md`, Facts): a dot is a fill
    /// with nothing on it, so `base-content/60` is its background. Written
    /// whole, as `badge` says why.
    pub fn dot(self) -> &'static str {
        match self {
            Status::Done => "status status-success",
            Status::Failed => "status status-error",
            Status::InFlight => "status status-info",
            Status::Waiting => "status status-warning",
            Status::Cancelled => "status status-neutral",
            Status::Paused => "status bg-base-content/60",
        }
    }

    /// The class that sets `--tone` to its colour, which the
    /// stylesheet's rules for a toned element read. Written whole, as
    /// `badge` says why.
    pub fn accent(self) -> &'static str {
        match self {
            Status::Done => "[--tone:var(--color-success)]",
            Status::Failed => "[--tone:var(--color-error)]",
            Status::InFlight => "[--tone:var(--color-info)]",
            Status::Waiting => "[--tone:var(--color-warning)]",
            Status::Cancelled => "[--tone:var(--color-neutral)]",
            Status::Paused => {
                "[--tone:color-mix(in_oklab,var(--color-base-content)_60%,transparent)]"
            }
        }
    }

    /// The class of text in its colour, such as a status's icon beside
    /// words (`docs/specs/board.md`, Item, Related items, and
    /// `docs/specs/components.md`, Transcript). Written whole, as `badge`
    /// says why.
    pub fn text(self) -> &'static str {
        match self {
            Status::Done => "text-success",
            Status::Failed => "text-error",
            Status::InFlight => "text-info",
            Status::Waiting => "text-warning",
            Status::Cancelled => "text-neutral",
            Status::Paused => "text-base-content/60",
        }
    }

    /// The classes of an alert in its colour: neutral, and `base-content/60`
    /// with no content colour, are daisyUI's alert with no colour of its
    /// own. Written whole, as `badge` says why.
    pub fn alert(self) -> &'static str {
        match self {
            Status::Done => "alert alert-success",
            Status::Failed => "alert alert-error",
            Status::InFlight => "alert alert-info",
            Status::Waiting => "alert alert-warning",
            Status::Cancelled => "alert",
            Status::Paused => "alert",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Item {
        number: Number,
        kind: ItemKind,
    },
    Time(Timestamp),
    /// `since` is when it started, if it is still running.
    Duration {
        seconds: u64,
        since: Option<Timestamp>,
    },
    /// A label, a branch, a glob or a command's name.
    Code(String),
}

impl Phrase {
    pub fn segments(&self) -> &[Segment] {
        &self.0
    }

    fn push(mut self, segment: Segment) -> Phrase {
        self.0.push(segment);
        self
    }

    #[must_use]
    pub fn text(self, text: impl Into<String>) -> Phrase {
        self.push(Segment::Text(text.into()))
    }

    #[must_use]
    pub fn issue(self, number: Number) -> Phrase {
        self.push(Segment::Item {
            number,
            kind: ItemKind::Issue,
        })
    }

    #[must_use]
    pub fn pull_request(self, number: Number) -> Phrase {
        self.push(Segment::Item {
            number,
            kind: ItemKind::PullRequest,
        })
    }

    #[must_use]
    pub fn time(self, at: Timestamp) -> Phrase {
        self.push(Segment::Time(at))
    }

    #[must_use]
    pub fn duration(self, seconds: u64, since: Option<Timestamp>) -> Phrase {
        self.push(Segment::Duration { seconds, since })
    }

    /// `span` as a duration, as 0 when it is negative, as one measured on
    /// clocks that disagree can be.
    #[must_use]
    pub fn span(self, span: TimeDelta, since: Option<Timestamp>) -> Phrase {
        self.duration(u64::try_from(span.num_seconds()).unwrap_or(0), since)
    }

    #[must_use]
    pub fn code(self, code: impl Into<String>) -> Phrase {
        self.push(Segment::Code(code.into()))
    }

    #[must_use]
    pub fn then(mut self, phrase: Phrase) -> Phrase {
        self.0.extend(phrase.0);
        self
    }

    /// `phrases` as a list: "a", "a and b", "a, b and c", with `conjunction`
    /// in place of "and".
    pub fn list(phrases: impl IntoIterator<Item = Phrase>, conjunction: &str) -> Phrase {
        let mut phrases: Vec<Phrase> = phrases.into_iter().collect();
        let Some(last) = phrases.pop() else {
            return Phrase::default();
        };
        if phrases.is_empty() {
            return last;
        }
        Phrase::join(phrases, ", ")
            .text(format!(" {conjunction} "))
            .then(last)
    }

    pub fn join(phrases: impl IntoIterator<Item = Phrase>, separator: &str) -> Phrase {
        phrases
            .into_iter()
            .enumerate()
            .fold(Phrase::default(), |joined, (i, phrase)| {
                let joined = if i == 0 {
                    joined
                } else {
                    joined.text(separator)
                };
                joined.then(phrase)
            })
    }
}

impl From<&str> for Phrase {
    fn from(text: &str) -> Phrase {
        Phrase::default().text(text)
    }
}

impl From<String> for Phrase {
    fn from(text: String) -> Phrase {
        Phrase::default().text(text)
    }
}

/// As the commands print it, so that what a tool returns reads as they do.
impl serde::Serialize for Phrase {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.0 {
            match segment {
                Segment::Text(text) => f.write_str(text)?,
                Segment::Item { number, .. } => write!(f, "{number}")?,
                Segment::Time(at) => write!(f, "{}", at.format("%Y-%m-%d %H:%M:%S UTC"))?,
                Segment::Duration { seconds, .. } => write_duration(f, *seconds)?,
                Segment::Code(code) => write!(f, "`{code}`")?,
            }
        }
        Ok(())
    }
}

/// `seconds` to its two largest units that are not zero, in narrow units, and
/// `0s` for none (`docs/specs/design_system.md`, Times, Durations): the twin of
/// `duration()` in `assets/linewright.js`.
fn write_duration(f: &mut fmt::Formatter<'_>, seconds: u64) -> fmt::Result {
    if seconds == 0 {
        return f.write_str("0s");
    }
    let balanced = [
        (seconds / 86_400, "d"),
        (seconds % 86_400 / 3_600, "h"),
        (seconds % 3_600 / 60, "m"),
        (seconds % 60, "s"),
    ];
    let largest = balanced.iter().filter(|(n, _)| *n > 0).take(2);
    for (i, (n, unit)) in largest.enumerate() {
        if i > 0 {
            f.write_str(" ")?;
        }
        write!(f, "{n}{unit}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `docs/specs/design_system.md`, Status: paused is `base-content/60`,
    /// cancelled `neutral`.
    #[test_log::test]
    fn each_status_has_the_colour_of_the_states_it_shows() {
        let statuses = [
            Status::Done,
            Status::Failed,
            Status::InFlight,
            Status::Waiting,
            Status::Cancelled,
            Status::Paused,
        ];

        assert_eq!(
            statuses.map(Status::badge),
            [
                "badge badge-success",
                "badge badge-error",
                "badge badge-info",
                "badge badge-warning",
                "badge badge-neutral",
                "badge badge-outline text-base-content/60",
            ]
        );
        assert_eq!(
            statuses.map(Status::dot),
            [
                "status status-success",
                "status status-error",
                "status status-info",
                "status status-warning",
                "status status-neutral",
                "status bg-base-content/60",
            ]
        );
        assert_eq!(
            statuses.map(Status::accent),
            [
                "[--tone:var(--color-success)]",
                "[--tone:var(--color-error)]",
                "[--tone:var(--color-info)]",
                "[--tone:var(--color-warning)]",
                "[--tone:var(--color-neutral)]",
                "[--tone:color-mix(in_oklab,var(--color-base-content)_60%,transparent)]",
            ]
        );
        assert_eq!(
            statuses.map(Status::alert),
            [
                "alert alert-success",
                "alert alert-error",
                "alert alert-info",
                "alert alert-warning",
                "alert",
                "alert",
            ]
        );
        assert_eq!(
            statuses.map(Status::text),
            [
                "text-success",
                "text-error",
                "text-info",
                "text-warning",
                "text-neutral",
                "text-base-content/60",
            ]
        );
    }
}
