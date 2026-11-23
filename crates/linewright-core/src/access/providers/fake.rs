//! Fakes of the identity providers, each served over HTTP on a port of
//! `127.0.0.1`, for the tests and `serve-fake`.

mod github;
mod google;

pub use github::{FakeGitHub, FakeGitHubAccount};
pub use google::{FakeAccount, FakeGoogle};
