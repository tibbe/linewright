//! The logos the web UI draws, which are their owners' own and none of the
//! icon set (`docs/specs/design_system.md`, Icons): Google's "G" and
//! GitHub's mark from Octicons, vendored beside this file with their
//! licences.

use linewright_core::access::providers::Provider;

use super::html::Html;
use super::icons::{IconSize, Marker, Read, drawn_svg};

/// The vendored SVG file of `provider`'s logo.
fn source(provider: Provider) -> &'static str {
    match provider {
        Provider::Google => include_str!("logos/google/g-logo.svg"),
        Provider::GitHub => include_str!("logos/octicons/mark-github-16.svg"),
    }
}

/// `provider`'s logo at `size-4`, in the colour of the text around it, with
/// the classes `class` too, read as `read` says and marked by `data-logo`
/// with the provider's key.
fn drawn(provider: Provider, class: Option<&str>, read: Read) -> Html {
    drawn_svg(
        source(provider),
        IconSize::Inline,
        class,
        read,
        Marker::Logo(provider.key()),
    )
}

/// The logo of a person's identity provider, which a screen reader reads as
/// the provider's name.
pub(super) fn identity_logo(provider: Provider) -> Html {
    drawn(provider, None, Read::Named(provider.name()))
}

/// GitHub's mark at `size-4`, with the classes `class` too, hidden from
/// screen readers.
pub(super) fn github_mark(class: &str) -> Html {
    drawn(Provider::GitHub, Some(class), Read::Hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn an_identity_logo_is_the_providers_own_read_as_its_name() {
        assert_eq!(
            identity_logo(Provider::GitHub).to_string(),
            r#"<svg class="icon size-4 shrink-0" viewBox="0 0 16 16" fill="currentColor" role="img" aria-label="GitHub" data-logo="github"><path d="M6.766 11.328c-2.063-.25-3.516-1.734-3.516-3.656 0-.781.281-1.625.75-2.188-.203-.515-.172-1.609.063-2.062.625-.078 1.468.25 1.968.703.594-.187 1.219-.281 1.985-.281.765 0 1.39.094 1.953.265.484-.437 1.344-.765 1.969-.687.218.422.25 1.515.046 2.047.5.593.766 1.39.766 2.203 0 1.922-1.453 3.375-3.547 3.64.531.344.89 1.094.89 1.954v1.625c0 .468.391.734.86.547C13.781 14.359 16 11.53 16 8.03 16 3.61 12.406 0 7.984 0 3.563 0 0 3.61 0 8.031a7.88 7.88 0 0 0 5.172 7.422c.422.156.828-.125.828-.547v-1.25c-.219.094-.5.156-.75.156-1.031 0-1.64-.562-2.078-1.609-.172-.422-.36-.672-.719-.719-.187-.015-.25-.093-.25-.187 0-.188.313-.328.625-.328.453 0 .844.281 1.25.86.313.452.64.655 1.031.655s.641-.14 1-.5c.266-.265.47-.5.657-.656"/></svg>"#,
        );
        let google = identity_logo(Provider::Google).to_string();
        assert!(
            google.starts_with(
                r##"<svg class="icon size-4 shrink-0" viewBox="0 0 48 48" fill="currentColor" role="img" aria-label="Google" data-logo="google"><path fill="#EA4335" "##
            ),
            "{google}"
        );
    }

    #[test_log::test]
    fn githubs_mark_takes_its_classes_and_is_hidden_from_screen_readers() {
        assert_eq!(
            github_mark("text-black dark:text-white").to_string(),
            r#"<svg class="icon size-4 shrink-0 text-black dark:text-white" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true" data-logo="github"><path d="M6.766 11.328c-2.063-.25-3.516-1.734-3.516-3.656 0-.781.281-1.625.75-2.188-.203-.515-.172-1.609.063-2.062.625-.078 1.468.25 1.968.703.594-.187 1.219-.281 1.985-.281.765 0 1.39.094 1.953.265.484-.437 1.344-.765 1.969-.687.218.422.25 1.515.046 2.047.5.593.766 1.39.766 2.203 0 1.922-1.453 3.375-3.547 3.64.531.344.89 1.094.89 1.954v1.625c0 .468.391.734.86.547C13.781 14.359 16 11.53 16 8.03 16 3.61 12.406 0 7.984 0 3.563 0 0 3.61 0 8.031a7.88 7.88 0 0 0 5.172 7.422c.422.156.828-.125.828-.547v-1.25c-.219.094-.5.156-.75.156-1.031 0-1.64-.562-2.078-1.609-.172-.422-.36-.672-.719-.719-.187-.015-.25-.093-.25-.187 0-.188.313-.328.625-.328.453 0 .844.281 1.25.86.313.452.64.655 1.031.655s.641-.14 1-.5c.266-.265.47-.5.657-.656"/></svg>"#,
        );
    }
}
