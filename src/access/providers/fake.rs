//! Fakes of the identity providers, each served over HTTP on a port of
//! `127.0.0.1`, for the tests and `serve-fake`.

mod google;

pub use google::{FakeAccount, FakeGoogle};
