# Contributing to Nemorous

Thanks for helping improve Nemorous. Please keep changes focused, tested and easy to review.

## Development setup

Use Rust 1.99+ and Git. Install the Xcode command-line tools for the macOS app. See the [platform build notes](platforms/README.md) for the Linux and Windows Rust entry points.

```bash
git clone https://github.com/iwangjie/Nemorous.git
cd Nemorous
scripts/prepare-nsh.sh
cargo test --locked --release --lib --test domain_git_tests --test headless_review_ui_tests
cargo build --locked --release --bin nemorous
```

Run the app with a disposable workspace:

```bash
NEMOROUS_WORKSPACE=/path/to/synthetic-workspace \
  NEMOROUS_CONFIG="$PWD/build/dev-nemorous.json" cargo run --locked --bin nemorous
```

`NEMOROUS_CONFIG` keeps development settings out of your normal profile.

## Pull requests

- Link the issue or describe the user-visible problem.
- Keep one logical change per pull request.
- Add or update tests for behaviour changes. UI changes should include a headless UI test.
- Run the Rust tests above and verify app or packaging changes with a macOS build.
- Do not include credentials, private repository names, customer data or real production screenshots.
- Screenshots and demos must use a synthetic workspace.

CI runs Rust unit, Git integration and headless UI tests on macOS and verifies the app bundle. Linux and Windows jobs check the portable Rust library.

## Code guidelines

- Follow existing package boundaries and naming.
- Prefer the standard library and existing project helpers over new dependencies.
- Use the existing Git helpers and keep Git process handling out of UI components.
- Keep application state in the Rust model. Run blocking Git operations off the UI thread and use the existing state updates to apply results.
- Bound external work with contexts, timeouts and result limits. This includes Git, LSP, AI and network calls.
- Destructive actions require an explicit confirmation and a clear error path.
- Do not silently swallow errors that affect repository state.
- Use comments to explain invariants and decisions.

## Tests

- Unit tests live with the Rust source under `src/`.
- Git integration and headless UI suites live in `tests/domain_git_tests.rs` and `tests/headless_review_ui_tests.rs`.
- With a Mac desktop session, run `NEMOROUS_NATIVE_GIT_LOG=1 cargo test --locked --release --test native_macos_window -- --nocapture` for the native Git review smoke test. It creates its own synthetic repository.
- Prefer deterministic fixtures and temporary Git repositories.
- Do not assert incidental wording or implementation details.

## Commit messages

Use a concise imperative subject, optionally with a body:

```
feat: add workspace onboarding
fix: preserve Windows LSP command paths
```

## Release process

Maintainers:

1. Update the version in `Cargo.toml` and `Cargo.lock` and add the matching section to `CHANGELOG.md`. Run the Rust tests and build the macOS app.
2. Run [Prepare signed macOS update](https://github.com/iwangjie/Nemorous/actions/workflows/macos-update.yml) from the release commit on `main`. It builds Apple Silicon and Intel packages and signs their update manifests with the existing key.
3. Prepare the GitHub release for `vX.Y.Z`. Include the two macOS installers, archives and signed manifests. Carry forward the v0.10.21 Windows and Linux installers, archives and signed manifests for both architectures. Those clients read `releases/latest/download/update-{windows,linux}-{amd64,arm64}.json`. Keep the [v0.10.21 release](https://github.com/iwangjie/Nemorous/releases/tag/v0.10.21) available for direct downloads.
4. Generate checksums and verify the macOS and legacy download and update URLs before promoting the release to latest.

The macOS manifest signer uses the Go standard library and can be built independently with Go 1.27.

```bash
mkdir -p build
GO111MODULE=off GOTOOLCHAIN=local go build -trimpath \
  -o build/sign-macos-update ./scripts/sign-macos-update/main.go
```

Go v0.10.21 still reads the static index at `plugins/registry.json` on `main`. Its bundled plugin source and documentation remain available at the historical tag.

## Security

Report vulnerabilities privately through GitHub Security Advisories. See [SECURITY.md](SECURITY.md).
