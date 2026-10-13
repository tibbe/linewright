use globset::GlobMatcher;

/// The glob that says which tracked files are the spec
/// (`docs/specs/sources_of_work.md`).
#[derive(Clone, Debug)]
pub struct SpecGlob {
    pattern: String,
    matcher: GlobMatcher,
}

impl SpecGlob {
    pub fn new(pattern: &str) -> Result<Self, globset::Error> {
        let matcher = globset::GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()?
            .compile_matcher();
        Ok(SpecGlob {
            pattern: pattern.to_string(),
            matcher,
        })
    }

    pub fn matches(&self, path: &str) -> bool {
        self.matcher.is_match(path)
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

impl Default for SpecGlob {
    fn default() -> Self {
        SpecGlob::new("docs/**/*.md").expect("the default spec glob parses")
    }
}

impl PartialEq for SpecGlob {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}
