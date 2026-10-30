//! Each back-door state is what its stage leaves, checked by running the
//! stage from the same start.

use crate::common::*;

#[tokio::test]
async fn a_ticket_in_review_is_what_the_implement_stage_leaves() {
    let mut front = World::new().await;
    front.implement_a_ticket().await;
    let mut back = World::new().await;

    back.ticket_in_review().await;

    assert_same_state(&front.store, &back.store).await;
}
