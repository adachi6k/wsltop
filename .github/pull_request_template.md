## Summary

Describe the problem and the focused change.

## User-visible and compatibility impact

Describe output, option, JSON, or accounting changes. State "none" when applicable.

## Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --locked --all-targets`
- [ ] `cargo clippy --locked --all-targets --all-features -- -D warnings`
- [ ] `cargo build --release --locked`
- [ ] Windows + WSL real-host testing completed or not applicable

List the real-host environment and scenarios tested when applicable.
