# Release Checklist

## Before publishing

- Create a GitHub repository for the project.
- Add the repository URL to `Cargo.toml` if you want crates.io metadata to point back to source.
- Run `cargo fmt`, `cargo test`, and `cargo build`.
- Run `cargo package --allow-dirty` and inspect the packaged file list.
- Make sure no session data or local development artifacts are accidentally included.
- Build release binaries for GitHub Releases so users can install `picocode` without running from source.
- Publish an `install` script alongside the release assets so users can run a one-line installer.

## Suggested first tag

- `v0.1.0`
- or `v0.1.0-alpha.1` if you want to signal that the UI and capability surface are still evolving

## Suggested publish flow

```bash
git init
git add .
git commit -m "Initial public release"
git tag v0.1.0
git push -u origin main --tags
cargo publish
```

## Notes

- The binary is `picocode`.
- The package name on crates.io is `picocode-cli`.
- The library crate name is `picocode`.
- The user-facing install path should be the prebuilt binary from GitHub Releases, not `cargo run`.
- The preferred installer pattern is `curl -fsSL .../install | bash`.
