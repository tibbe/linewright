---
on:
  schedule: "0 7 * * 1"
timeout: 30m
max-issues: 10
---
Check whether the dependencies have releases at least 7 days old that
`Cargo.toml` or `Cargo.lock` does not yet use, and file issues to upgrade to
them.

1. List the newer releases, both those `Cargo.lock` can move to and those that
   need a new version requirement in `Cargo.toml` (`Unchanged ... (available:
   ...)`):

   ```sh
   cargo update --dry-run --verbose
   ```

2. Read each release's date, `created_at`, from crates.io, which refuses
   requests without a user agent. For each crate, keep the newest release
   that is not yanked and came out 7 or more days ago, if it is newer than the
   one in use.

   ```sh
   curl -sSf -A 'linewright (https://github.com/tibbe/linewright)' \
     https://crates.io/api/v1/crates/NAME/versions
   ```

3. Leave out a release when any of these holds:
   - It needs a new version requirement, and the other dependencies do not
     support it yet, such as a crate whose types we pass to another
     dependency that still uses the older version.
   - An open issue already upgrades the crate, whatever version it names.

4. File `ready-for-agent` issues for the rest:
   - One titled "Update the locked dependencies" for all the releases
     `Cargo.lock` can move to, listing each crate and version, and saying to
     run `cargo update NAME --precise VERSION` for each, so no release younger
     than 7 days gets in.
   - One titled "Upgrade CRATE to VERSION" for each release that needs a new
     version requirement, saying to change it in `Cargo.toml` and adapt the
     code to the release's breaking changes, with a link to its changelog.
     Crates that must move together, such as ones sharing a trait crate, go
     in one issue.
