# Contributing to Nemorous

Thanks for helping improve Nemorous. Please keep changes focused, tested and easy to review.

## Development setup

Requirements:
- Go 1.27+
- Git
- macOS, Windows or Linux

```bash
git clone https://github.com/iwangjie/Nemorous.git
cd Nemorous
go mod download
go test ./...
go vet ./...
go tool mygo vet .
```

Run the app with a disposable workspace:

```bash
NEMOROUS_WORKSPACE=/path/to/workspace go run .
```

`NEMOROUS_CONFIG=/tmp/nemorous.json` keeps local configuration out of your normal profile.

## Pull requests

- Link the issue or describe the user-visible problem.
- Keep one logical change per pull request.
- Add or update tests for behaviour changes. UI changes should include a headless UI test.
- Run `gofmt`, `go vet`, `go tool mygo vet .` and `go test ./...` before opening the PR.
- Do not include credentials, private repository names, customer data or real production screenshots.
- Screenshots and demos must use a synthetic workspace.

CI runs on macOS, Ubuntu and Windows. Linux also runs the backend race suite.

## Code guidelines

- Follow existing package boundaries and naming.
- Prefer the standard library and existing project helpers over new dependencies.
- Git operations belong in `internal/gitops`; UI code must not shell out directly.
- Never store `ui.Element` or `*ui.Context` in `App`. Keep state in `App` and rebuild UI each frame.
- Background work must return to the UI through `runUI`.
- Bound external work with contexts, timeouts and result limits. This includes Git, LSP, AI and network calls.
- Destructive actions require an explicit confirmation and a clear error path.
- Do not silently swallow errors that affect repository state.
- Comments explain invariants or why; avoid narrating obvious code.

## Tests

- Unit tests: `internal/*`.
- Headless UI tests: repository root `*_test.go`.
- End-to-end plugin tests live with each plugin.
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

1. Update `CHANGELOG.md` and `mygo.json` version.
2. Run the full test suite and `go tool mygo build`.
3. Commit, tag `vX.Y.Z`, and push the tag.
4. The Release workflow builds all platforms, verifies tests, publishes signed update manifests and attaches checksums.

## Security

Report vulnerabilities privately through GitHub Security Advisories. See [SECURITY.md](SECURITY.md).
