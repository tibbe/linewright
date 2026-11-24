use std::time::Duration;

use crate::store::local::{Paused, Write};

pub fn clone_of(branch: &str) -> Write {
    Write::Clone {
        branch: branch.into(),
    }
}

/// Waits until `paused`'s clone is made, failing past 30 seconds.
pub async fn reached(paused: &mut Paused) {
    if tokio::time::timeout(Duration::from_secs(30), paused.reached())
        .await
        .is_err()
    {
        panic!("the clone awaited was never made");
    }
}
