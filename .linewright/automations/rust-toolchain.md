---
on:
  schedule: "0 6 * * 1"
session-timeout: 15m
max-issues: 1
---
Check whether `rust-toolchain.toml` pins the latest stable Rust, and file an
issue to upgrade it when it does not.

1. Read the latest stable release from the stable channel's manifest. Its
   release date is the manifest's top-level `date`, not the commit date in
   `version`.

   ```sh
   curl -sSf https://static.rust-lang.org/dist/channel-rust-stable.toml
   ```

2. File nothing when any of these holds:
   - `channel` in `rust-toolchain.toml` already names that release.
   - The release came out less than 3 days ago.
   - An issue to upgrade the toolchain is open, whatever version it names.

3. Otherwise file one `ready-for-agent` issue titled "Upgrade the Rust
   toolchain to VERSION", saying to set `channel` in `rust-toolchain.toml`
   to VERSION.
