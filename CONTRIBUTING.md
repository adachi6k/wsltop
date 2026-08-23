# Contributing to wsltop

Thank you for helping improve `wsltop`. Small, focused issues and pull requests are easiest to review.

## Build and checks

Use a current stable Rust toolchain and the tracked dependency lockfile:

```console
cargo build --locked
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --release --locked
```

Run `cargo run --locked -- --help` and `cargo run --locked -- --version` when changing CLI behavior or package metadata.

## Branches and pull requests

- Create a topic branch from the latest `main`.
- Keep each commit and pull request focused on one coherent change.
- Do not include unrelated formatting or generated files other than `Cargo.lock` when dependency resolution changes.
- Describe user-visible behavior, compatibility impact, tests performed, and remaining limitations.
- Add or update unit tests and documentation for behavior changes.
- Ensure formatting, tests, clippy with warnings denied, and the release build pass before requesting review.

## Windows and WSL validation

Ubuntu CI cannot exercise Windows interoperability or provide meaningful WSLC, Docker Desktop, multi-distribution, `vmmem*`, and Task Manager comparisons. Collector or accounting changes should be tested on a real Windows 11 + WSL2 host using [the validation plan](docs/test-plan.md). Include relevant Windows, WSL, distro, WSLC, Docker, and `wsltop` versions in the pull request.

Never include credentials, private container data, or other sensitive output in an issue or pull request.
