# WinGet distribution

Package ID: `Adachi6k.wsltop`. Initial submission:
[microsoft/winget-pkgs#430402](https://github.com/microsoft/winget-pkgs/pull/430402)
for v0.4.0. Community registration is pending; do not
advertise `winget install --id Adachi6k.wsltop --exact` as available until the
upstream PR is merged and the package appears in the WinGet source.

## Manifest generation

The official GitHub Release ZIP is used unchanged as a nested portable package.
The command alias is `wsltop`. Minimum Windows version is Windows 11
(`10.0.22000.0`); the description explains the required primary WSL2 distribution.
Installing the package does not provision WSL or a distribution.

The [initial Windows CI run](https://github.com/adachi6k/wsltop/actions/runs/34032216509)
passed generation, native validation, user-scope installation, installed
version/help, and local-manifest uninstallation. The Microsoft submission checks
also passed; moderator approval/source indexing remain external steps.

```console
python3 scripts/winget.py --tag v0.4.0 --output generated
```

Requires Python 3.10+ and authenticated GitHub CLI. Generation accepts only a
published stable version, downloads the fixed release ZIP and SHA-256 sidecar,
checks the hash and exact versioned ZIP layout, and generates three schema 1.10.0
YAML files. A changed ZIP layout or checksum fails instead of silently submitting
an invalid installer. The initial manifest is under `packaging/winget/manifests`.
Future manifests are regenerated from each release; the nested executable path
changes with the version, not just the URL and hash.

On Windows, validate with `winget validate --manifest <version-directory>`.
Test with `winget install --manifest <version-directory> --scope user`, then
verify the installed alias's version/help and uninstall the test package.
Local manifest installation requires the administrator-controlled
`LocalManifestFiles` setting. The CI test enables it only on its disposable runner.

## Automation

`.github/workflows/winget.yml` runs after the **Release** workflow completes
successfully for a pushed stable tag. It uses `workflow_run`, because releases
created with `GITHUB_TOKEN` do not trigger another ordinary `release` workflow.
The originating repository and resolved tag commit are checked before generation.
Release dry runs and PR runs do not submit anything upstream.

Every run generates manifests, validates with native WinGet, tests portable
install/version/help/uninstall on Windows, and uploads a manifest artifact.
PRs that change this integration run the same generation and Windows validation
against the already published v0.4.0 release, without access to submission secrets.
Manual `workflow_dispatch` accepts a tag and defaults to validation only.

To enable automatic upstream PRs, configure repository Actions secret
`WINGET_SUBMISSION_TOKEN` with a dedicated GitHub token capable of creating a
fork/branches and PRs in public repositories (a classic token with `public_repo`
scope, as described by Microsoft). Do not reuse the repository's `GITHUB_TOKEN`:
it is scoped to this repository. No token is stored in scripts or manifests.
Without the secret, generation/testing/artifact upload still run and the summary
explains that submission was skipped. Set the secret via GitHub Settings rather
than putting it in an issue, PR, or command argument.

After validation, submission can also be run with a locally authenticated `gh`:

```console
python3 scripts/submit-winget.py --tag v0.4.0 --manifest-dir generated/manifests/a/Adachi6k/wsltop/0.4.0
```

Submission checks for an existing upstream version/open PR, then creates a branch
in the authenticated user's winget-pkgs fork and submits only the three manifest
files. Reruns reuse identical branches; differing content requires manual review.
Existing versions are never replaced. Microsoft may request CLA acceptance or
manifest changes; this workflow does not approve legal agreements or merge PRs.
Community acceptance and source indexing happen independently of wsltop releases.

After acceptance: `winget show --id Adachi6k.wsltop --exact --source winget`,
then test installation/upgrade from that public source before updating README.

References: [manifest authoring](https://learn.microsoft.com/windows/package-manager/package/manifest),
[first contribution](https://github.com/microsoft/winget-pkgs/blob/master/doc/FirstContribution.md),
[WinGetCreate submission](https://github.com/microsoft/winget-create/blob/main/doc/submit.md).
