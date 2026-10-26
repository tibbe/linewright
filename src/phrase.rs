use std::fmt;

use chrono::TimeDelta;

use crate::domain::{Number, Timestamp};

/// Wording the monitoring commands show, with the items, times, durations
/// and labels it names kept apart from its text, so that each can be shown as
/// suits where it is shown. `Display` gives it as the commands print it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Phrase(Vec<Segment>);

/// What an outcome or a state shows as, each with its own icon
/// (`docs/specs/web/design_system.md`, Status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Failure,
    InProgress,
    Pending,
    Cancelled,
    Paused,
}

impl Tone {
    /// The daisyUI colour it shows in, as the class of a badge in it: a badge
    /// gives each colour its own colour of text, which reads on it in either
    /// theme. Written whole, since Tailwind finds the classes the stylesheet
    /// needs by scanning the sources' text
    /// (<https://tailwindcss.com/docs/detecting-classes-in-source-files>).
    pub fn colour(self) -> &'static str {
        match self {
            Tone::Success => "badge-success",
            Tone::Failure => "badge-error",
            Tone::InProgress => "badge-info",
            Tone::Pending => "badge-warning",
            Tone::Cancelled | Tone::Paused => "badge-neutral",
        }
    }

    /// The class that sets `--tone` to its daisyUI colour, which the
    /// stylesheet's rules for a toned element read. Written whole, as
    /// `colour` says why.
    pub fn accent(self) -> &'static str {
        match self {
            Tone::Success => "[--tone:var(--color-success)]",
            Tone::Failure => "[--tone:var(--color-error)]",
            Tone::InProgress => "[--tone:var(--color-info)]",
            Tone::Pending => "[--tone:var(--color-warning)]",
            Tone::Cancelled | Tone::Paused => "[--tone:var(--color-neutral)]",
        }
    }

    /// The classes of an alert in its colour: neutral is daisyUI's alert with
    /// no colour of its own. Written whole, as `colour` says why.
    pub fn alert(self) -> &'static str {
        match self {
            Tone::Success => "alert alert-success",
            Tone::Failure => "alert alert-error",
            Tone::InProgress => "alert alert-info",
            Tone::Pending => "alert alert-warning",
            Tone::Cancelled | Tone::Paused => "alert",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Item(Number),
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
    pub fn item(self, number: Number) -> Phrase {
        self.push(Segment::Item(number))
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

impl From<Number> for Phrase {
    fn from(number: Number) -> Phrase {
        Phrase::default().item(number)
    }
}

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for segment in &self.0 {
            match segment {
                Segment::Text(text) => f.write_str(text)?,
                Segment::Item(number) => write!(f, "{number}")?,
                Segment::Time(at) => write!(f, "{}", at.format("%Y-%m-%d %H:%M:%S UTC"))?,
                Segment::Duration { seconds, .. } => write_duration(f, *seconds)?,
                Segment::Code(code) => write!(f, "`{code}`")?,
            }
        }
        Ok(())
    }
}

/// `seconds` to its two largest units that are not zero, in narrow units, and
/// `0s` for none (`docs/specs/design_system.md`, Time, Durations): the twin of
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

    /// `docs/specs/web/design_system.md`, Status.
    #[test]
    fn each_tone_has_the_daisyui_colour_of_the_states_it_shows() {
        let tones = [
            Tone::Success,
            Tone::Failure,
            Tone::InProgress,
            Tone::Pending,
            Tone::Cancelled,
            Tone::Paused,
        ];

        assert_eq!(
            tones.map(Tone::colour),
            [
                "badge-success",
                "badge-error",
                "badge-info",
                "badge-warning",
                "badge-neutral",
                "badge-neutral",
            ]
        );
        assert_eq!(
            tones.map(Tone::alert),
            [
                "alert alert-success",
                "alert alert-error",
                "alert alert-info",
                "alert alert-warning",
                "alert",
                "alert",
            ]
        );
    }
}
