# Scoop Main proposal

[`wsltop.json`](wsltop.json) is a proposed `ScoopInstaller/Main/bucket/wsltop.json`
for the public v0.5.0 Windows x64 ZIP. It is not yet in Scoop Main.

The ZIP contains `wsltop-v0.5.0-x86_64-pc-windows-msvc/wsltop.exe`.
`extract_dir` removes that versioned directory, so `bin: "wsltop.exe"`
creates the `wsltop` command. The manifest uses the release's SHA256 sidecar
for autoupdate and updates `extract_dir` along with the download URL.
Windows 11 and a usable WSL2 distribution are runtime requirements.

With Scoop already installed, a local manifest can be tested from this checkout:

```powershell
scoop install .\packaging\scoop\wsltop.json
wsltop --version
wsltop --help
scoop uninstall wsltop
```

## Upstream submission status

The [Main criteria](https://github.com/ScoopInstaller/Scoop/wiki/Criteria-for-including-apps-in-the-main-bucket)
call for a reasonably well-known developer tool, giving 500 stars and 150 forks
as a benchmark. On 2026-09-12, wsltop has 5 stars and 0 forks. Its current reach
does not meet that benchmark; manifest validation alone does not establish
eligibility for Main.

The [contribution guide](https://github.com/ScoopInstaller/.github/blob/main/.github/CONTRIBUTING.md)
and [PR template](https://github.com/ScoopInstaller/Main/blob/master/.github/pull_request_template.md)
also require a prior issue and maintainer discussion. No wsltop issue or PR was
found in Main when checked on 2026-09-12. No upstream issue or PR has been sent.

[`PACKAGE_REQUEST.md`](PACKAGE_REQUEST.md) provides the proposed eligibility
discussion, and [`PR.md`](PR.md) contains the subsequent PR title/body. After
maintainer agreement, copy only `wsltop.json` to `bucket/wsltop.json` in a Main
checkout and link the approved issue. Keep these proposal documents in wsltop.

## Validation

See [distribution validation](../../docs/distribution-v0.5.0.md) for executed
checks and tool versions. To repeat the upstream checks in a Scoop Main checkout
with Scoop, Pester and BuildHelpers installed:

```powershell
.\bin\formatjson.ps1 wsltop
.\bin\checkver.ps1 wsltop
.\bin\checkurls.ps1 wsltop -Timeout 60
.\bin\checkver.ps1 wsltop -Version 0.5.0 -ForceUpdate
.\bin\test.ps1
```

The checked-in manifest uses Scoop's four-space JSON formatting and CRLF line
endings; `.gitattributes` preserves the line endings on checkout.
