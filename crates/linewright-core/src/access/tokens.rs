//! The secrets of sign-ins: each 32 random bytes, base64url-encoded, and
//! kept only as its SHA-256, so that the database holds nothing that
//! signs a browser in.

use base64::Engine as _;
use sha2::Digest as _;

/// 32 random bytes, base64url-encoded without padding.
///
/// # Panics
///
/// When the system gives no random bytes.
pub(crate) fn random_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("the system gives random bytes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The SHA-256 of `token`, as the database keeps it.
pub(crate) fn hashed(token: &str) -> Vec<u8> {
    sha2::Sha256::digest(token.as_bytes()).to_vec()
}

/// The PKCE code challenge of `verifier` by the `S256` method.
/// <https://www.rfc-editor.org/rfc/rfc7636#section-4.2>
pub(crate) fn s256(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier))
}

/// The `Set-Cookie` of the cookie `name` holding `value`, which the browser
/// keeps for `max_age`, sends to the whole host and no other, sends only
/// over HTTPS or to a loopback address, and keeps from scripts and from
/// cross-site requests but top-level navigations.
/// <https://www.rfc-editor.org/rfc/rfc6265bis#name-the-__host-prefix>
pub(crate) fn host_cookie(name: &str, value: &str, max_age: chrono::TimeDelta) -> String {
    format!(
        "{name}={value}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Lax",
        max_age.num_seconds()
    )
}
