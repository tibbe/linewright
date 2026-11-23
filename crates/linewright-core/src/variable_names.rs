//! A list of variable names, as a setting such as `session-pass-env` gives it
//! (`docs/specs/operation.md`, "Configuration").

use crate::layer;

/// `pattern` is a variable's name, or a prefix when it ends in `*`.
pub fn matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => name == pattern,
    }
}

/// Refuses `name` with a `*` anywhere but at its end, naming `setting`, the
/// setting it is given in.
pub fn check(setting: &str, name: &str) -> Result<(), String> {
    if name.trim_end_matches('*').contains('*') {
        return Err(format!(
            "the setting `{setting}` names `{name}`: a `*` can only end a name"
        ));
    }
    Ok(())
}

/// The names in `names` separated by commas, the form of a setting such as
/// `session-pass-env` as a flag or variable.
pub fn comma_separated(names: &str) -> impl Iterator<Item = String> {
    names
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
}

/// Variable names, each a prefix when it ends in `*`, and with no `*`
/// elsewhere.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VariableNames {
    patterns: Vec<String>,
}

impl VariableNames {
    /// Refuses a name as [`check`] does.
    pub fn new(
        setting: &str,
        names: impl IntoIterator<Item = String>,
    ) -> Result<VariableNames, String> {
        let patterns: Vec<String> = names.into_iter().collect();
        for name in &patterns {
            check(setting, name)?;
        }
        Ok(VariableNames { patterns })
    }

    /// Whether one of the names is `name`, or a prefix of it.
    pub fn matches(&self, name: &str) -> bool {
        self.patterns.iter().any(|pattern| matches(pattern, name))
    }

    /// These names and `more`'s, a layer below them, as a list combines.
    #[must_use]
    pub fn and(&self, more: &VariableNames) -> VariableNames {
        VariableNames {
            patterns: layer::list([&self.patterns, &more.patterns], |name| name)
                .into_iter()
                .cloned()
                .collect(),
        }
    }

    /// The names that are not prefixes.
    pub fn exact(&self) -> impl Iterator<Item = &str> {
        self.patterns
            .iter()
            .map(String::as_str)
            .filter(|pattern| !pattern.ends_with('*'))
    }
}
