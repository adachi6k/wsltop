# Proposed package-request title

[Request]: wsltop

# Proposed request

Would wsltop be eligible for inclusion in Main?

- Project: https://github.com/adachi6k/wsltop
- Stable release: https://github.com/adachi6k/wsltop/releases/tag/v0.5.0
- License: MIT
- Type: portable, non-GUI developer CLI
- Platform: Windows x64; Windows 11 and a usable WSL2 distribution required
- Purpose: monitor Windows, WSL2, Docker and WSLC resource usage together
- Installation: versioned ZIP, one portable executable, no installation scripts
- Updates: GitHub checkver, versioned URL/directory and SHA256 sidecar

The project currently has 5 stars and 0 forks (2026-09-12), below the popularity
benchmark in the Main criteria. We would like to confirm eligibility before
opening a manifest PR.

A v0.5.0 manifest is prepared and has passed the official Scoop schema/style
tests, URL/checkver/autoupdate checks, local installation, shim version/help and
uninstallation. If the package is accepted, the PR will add only
`bucket/wsltop.json`.
