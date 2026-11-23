use std::path::PathBuf;

use super::runs::ended_done;
use crate::runs::{Recorded, Started};

pub fn run(id: &str, start: &str, sessions: usize) -> Recorded {
    ended_done(
        Started::new(id.parse().unwrap(), None).unwrap(),
        start,
        (0..sessions)
            .map(|n| PathBuf::from(format!("sessions/{id}/{n}")).into())
            .collect(),
    )
}
