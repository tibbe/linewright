use std::panic::Location;
use std::time::Duration;

use crate::store::local::{Paused, Write};

pub fn clone_of(branch: &str) -> Write {
    Write::Clone {
        branch: branch.into(),
    }
}

/// Waits until `paused`'s clone is made, failing past 30 seconds.
#[track_caller]
pub fn reached(paused: &mut Paused) -> impl Future<Output = ()> + '_ {
    let caller = Location::caller();
    async move {
        if tokio::time::timeout(Duration::from_secs(30), paused.reached())
            .await
            .is_err()
        {
            panic!("the clone awaited at {caller} was never made");
        }
    }
}
