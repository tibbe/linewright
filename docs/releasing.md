# Releasing

A release is a `vX.Y.Z` tag on `main`. Pushing it publishes the base image as
`X.Y.Z`, `X.Y` and `latest`, and a GitHub Release.

1. If `version` in `Cargo.toml` is not the one to release, change it in a pull
   request, with `Cargo.lock` updated by `cargo check`, and merge it.
2. Tag the merge commit and push the tag (only admins can):

   ```sh
   git tag -a v0.2.0 -m v0.2.0 && git push origin v0.2.0
   ```

The workflow refuses a tag that is not `v` followed by the version in
`Cargo.toml`. If it fails, re-run it; if the fix needs a commit, release that
as the next patch version.
