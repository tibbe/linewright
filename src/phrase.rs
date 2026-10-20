use std::fmt;

use crate::domain::{Number, Timestamp};

/// Wording the monitoring commands show, with the items, times, durations
/// and labels it names kept apart from its text, so that each can be shown as
/// suits where it is shown. `Display` gives it as the commands print it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Phrase(Vec<Segment>);

/// The check state on GitHub an outcome or a state maps to
/// (`docs/specs/design_system.md`, Status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Failure,
    InProgress,
    Pending,
    Cancelled,
}

impl Tone {
    /// As the web UI's stylesheet names it.
    pub fn name(self) -> &'static str {
        match self {
            Tone::Success => "success",
            Tone::Failure => "failure",
            Tone::InProgress => "in-progress",
            Tone::Pending => "pending",
            Tone::Cancelled => "cancelled",
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
                Segment::Duration { seconds, .. } => {
                    let duration = std::time::Duration::from_secs(*seconds);
                    write!(f, "{}", humantime::format_duration(duration))?;
                }
                Segment::Code(code) => write!(f, "`{code}`")?,
            }
        }
        Ok(())
    }
}
