//! What Linewright's HTTP clients share.

/// A builder of HTTP clients that trust the certificate authorities of the
/// host's store alone (`docs/specs/operation.md`, "Certificate
/// authorities").
pub fn client_builder_trusting_the_host_store_alone() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
}

/// The URL of the next page a `Link` header gives
/// (<https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api#using-link-headers>).
pub fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        (rel.trim() == "rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}
