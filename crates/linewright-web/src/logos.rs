//! The logos the web UI draws, which are their owners' own and none of the
//! icon set (`docs/specs/design_system.md`, Icons): Google's "G" and
//! GitHub's mark from Octicons, vendored beside this file with their
//! licences, and Sign in's "G", the spec's own `google-g.svg`.

use std::sync::LazyLock;

use linewright_core::access::providers::Provider;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use super::html::{self, Html};
use super::icons::{DrawnSize, Fill, IconSize, Marker, Read, drawn_svg};

/// The vendored SVG file of `provider`'s logo.
fn source(provider: Provider) -> &'static str {
    match provider {
        Provider::Google => include_str!("logos/google/g-logo.svg"),
        Provider::GitHub => include_str!("logos/octicons/mark-github-16.svg"),
    }
}

/// `provider`'s logo in `size`, filled with `fill`, read as `read` says
/// and marked by `data-logo` with the provider's key.
fn drawn(provider: Provider, size: impl DrawnSize, fill: Fill, read: Read) -> Html {
    drawn_svg(
        source(provider),
        size,
        None,
        fill,
        read,
        Marker::Logo(provider.key()),
    )
}

/// The logo of a person's identity provider, in a line of text and in its
/// colour, which a screen reader reads as the provider's name.
pub(super) fn identity_logo(provider: Provider) -> Html {
    drawn(
        provider,
        IconSize::Text,
        Fill::CurrentColor,
        Read::Named(provider.name()),
    )
}

/// The "G" on Google's button on Sign in (`docs/specs/access.md`, Sign in),
/// the spec's own, which [`source`]'s Google logo is not.
const SIGN_IN_G: &str = include_str!("../../../docs/specs/assets/google-g.svg");

/// What a `data:` URI percent-encodes of an SVG file: what a URI or an HTML
/// attribute can't hold as it is, and `%`, `#` and `&`.
const NOT_IN_A_DATA_URI: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'&')
    .add(b'\'')
    .add(b'<')
    .add(b'>')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// [`SIGN_IN_G`] as a `data:` URI.
static SIGN_IN_G_URI: LazyLock<String> = LazyLock::new(|| {
    format!(
        "data:image/svg+xml,{}",
        utf8_percent_encode(SIGN_IN_G.trim(), NOT_IN_A_DATA_URI)
    )
});

/// The size of a mark on Sign in's buttons (`docs/specs/access.md`, Sign
/// in), which is a logo and so not sized as an icon of the set in a `btn`.
#[derive(Clone, Copy)]
struct SignInMarkSize;

impl DrawnSize for SignInMarkSize {
    fn class(self) -> &'static str {
        "size-4"
    }
}

/// The mark before the label of `provider`'s button on Sign in, in
/// [`SignInMarkSize`] and hidden from screen readers, since the label names
/// the provider: Google's "G" and GitHub's mark in white
/// (`docs/specs/access.md`, Sign in).
pub(super) fn sign_in_mark(provider: Provider) -> Html {
    match provider {
        Provider::Google => html::built(format!(
            r#"<img src="{src}" width="16" height="16" class="{size} shrink-0" alt="" data-logo="{key}">"#,
            src = *SIGN_IN_G_URI,
            size = SignInMarkSize.class(),
            key = provider.key(),
        )),
        Provider::GitHub => drawn(provider, SignInMarkSize, Fill::White, Read::Hidden),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test]
    fn an_identity_logo_is_the_providers_own_read_as_its_name() {
        assert_eq!(
            identity_logo(Provider::GitHub).to_string(),
            r#"<svg class="size-[1em] inline-block align-[calc(0.5cap-0.5em)] shrink-0" viewBox="0 0 16 16" fill="currentColor" role="img" aria-label="GitHub" data-logo="github"><path d="M6.766 11.328c-2.063-.25-3.516-1.734-3.516-3.656 0-.781.281-1.625.75-2.188-.203-.515-.172-1.609.063-2.062.625-.078 1.468.25 1.968.703.594-.187 1.219-.281 1.985-.281.765 0 1.39.094 1.953.265.484-.437 1.344-.765 1.969-.687.218.422.25 1.515.046 2.047.5.593.766 1.39.766 2.203 0 1.922-1.453 3.375-3.547 3.64.531.344.89 1.094.89 1.954v1.625c0 .468.391.734.86.547C13.781 14.359 16 11.53 16 8.03 16 3.61 12.406 0 7.984 0 3.563 0 0 3.61 0 8.031a7.88 7.88 0 0 0 5.172 7.422c.422.156.828-.125.828-.547v-1.25c-.219.094-.5.156-.75.156-1.031 0-1.64-.562-2.078-1.609-.172-.422-.36-.672-.719-.719-.187-.015-.25-.093-.25-.187 0-.188.313-.328.625-.328.453 0 .844.281 1.25.86.313.452.64.655 1.031.655s.641-.14 1-.5c.266-.265.47-.5.657-.656"/></svg>"#,
        );
        let google = identity_logo(Provider::Google).to_string();
        assert!(
            google.starts_with(
                r##"<svg class="size-[1em] inline-block align-[calc(0.5cap-0.5em)] shrink-0" viewBox="0 0 48 48" fill="currentColor" role="img" aria-label="Google" data-logo="google"><path fill="#EA4335" "##
            ),
            "{google}"
        );
    }
}
