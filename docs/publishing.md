# Release publishing

The `Release` workflow in `.github/workflows/release.yml` checks the version tag
against `Cargo.toml`, builds and verifies Windows/Linux archives, validates the
Cargo package, publishes GitHub Release assets, then publishes to crates.io.
Only a `v*` tag push publishes. Pull requests and `workflow_dispatch` validate
without publishing or requesting a crates.io token. Prerelease tags also publish
their matching Cargo prerelease version.

## One-time setup

An owner of the existing `wsltop` crate must configure
[Trusted Publishing](https://crates.io/docs/trusted-publishing).

1. In the GitHub repository settings, create an environment named `crates-io`.
   Under deployment branches and tags, choose selected branches and tags and add
   a **tag** rule for `v*` (not a branch rule). Optionally require a reviewer.
2. On crates.io, open `wsltop` -> Settings -> Trusted Publishing, add a GitHub
   publisher, and enter these exact values:

   | Field | Value |
   | --- | --- |
   | Repository owner | `adachi6k` |
   | Repository name | `wsltop` |
   | Workflow filename | `release.yml` |
   | Environment | `crates-io` |

   The workflow field is the filename, not `.github/workflows/release.yml` and
   not the workflow's display name (`Release`). The environment must match the
   job's `environment` value.
3. Complete both settings before pushing the next release tag. No
   `CARGO_REGISTRY_TOKEN` repository secret is required. If an older publishing
   token exists, remove its secret and revoke the token after the first
   successful Trusted Publishing release, provided nothing else uses it.

Only the `publish-crate` job receives `id-token: write`. It verifies the package
before requesting a short-lived token with
[`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action).
The token is passed only to the publish step; the action revokes it when the job
finishes. Renaming the workflow or environment requires updating crates.io too.

## Releasing

1. Update `Cargo.toml`, `Cargo.lock`, and `CHANGELOG.md` for a new version, complete
   the [release checks](test-plan.md), and merge the changes.
2. Push the matching `v<version>` tag on the release commit. Do not move an
   existing published tag. Version 1.0.0 is already published; use a new version
   for the first Trusted Publishing release.
3. Check the `Release` workflow and approve the `crates-io` environment if a
   reviewer is configured. Confirm both GitHub assets and the crates.io version.

The crates.io job waits for GitHub assets so that prebuilt downloads are
available before the new crate is published. The two services are not an atomic
transaction: a crates.io failure can leave a valid GitHub Release in place.

## Failures and retries

- For authentication failures, check all four publisher fields, the environment
  tag rule, and the job's `id-token: write` permission. Fix settings and use
  GitHub Actions **Re-run failed jobs** on the original tag run.
- `workflow_dispatch` is validation-only; it does not retry publication.
- After a timeout or ambiguous upload result, check whether the version exists
  on crates.io before retrying. Published versions cannot be overwritten. If it
  is already present, do not republish it or move the tag. A rerun that attempts
  an already published version fails explicitly rather than masking the error.
- Dry-run checks validate packaging and compilation, not OIDC authorization.
  The first real tag publication is required to verify the complete exchange.
