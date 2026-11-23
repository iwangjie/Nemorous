## Problem

<!-- What user-visible problem does this solve? -->

## Change

<!-- The smallest useful description of the implementation. -->

## Verification

- [ ] `cargo test --locked --release --lib --test domain_git_tests --test headless_review_ui_tests`
- [ ] `cargo build --locked --release --bin nemorous` on macOS for app or packaging changes
- [ ] Added/updated tests for behaviour changes
- [ ] UI changes include a headless UI test
- [ ] No secrets, private repository names or production screenshots

## Risk

<!-- Destructive operations, migrations, platform-specific behaviour, etc. -->
