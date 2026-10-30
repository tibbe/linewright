//! Attachments, as Linewright uploads them for the Markdown it posts.

use anyhow::{Result, ensure};

use crate::check::contracts;
use crate::sandbox::{PNG, Sandbox};

contracts![an_attached_file_gives_the_address_to_link_it_by];

async fn an_attached_file_gives_the_address_to_link_it_by(sandbox: Sandbox) -> Result<()> {
    let file = sandbox.scratch().join("shot.png");
    tokio::fs::write(&file, PNG).await?;

    let address = sandbox
        .store()
        .attach(&file, "shot.png", "image/png")
        .await?;

    // Linewright puts it in place of a path, as a Markdown link's
    // destination (<https://spec.commonmark.org/0.31.2/#link-destination>).
    ensure!(
        !address.is_empty() && !address.contains(char::is_whitespace),
        "the address {address:?} is not one to link by"
    );
    Ok(())
}
