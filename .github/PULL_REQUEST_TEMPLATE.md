## Summary

Brief description of what this PR does.

## Type of change

- [ ] feat (new feature)
- [ ] fix (bug fix)
- [ ] docs (documentation only)
- [ ] refactor (no functional change)
- [ ] perf (performance)
- [ ] test (add/update tests)
- [ ] chore (build/infra/CI)
- [ ] ui (visual/interaction changes)
- [ ] breaking change (incompatible behavior)

## Related issues

Closes #

## How to test

Step-by-step instructions a reviewer can follow. Include any tool
prerequisites (e.g. `shellcheck`, `namcap`, `fakeroot`) if the change
touches the Validate step.

## Checklist

- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo check` compiles
- [ ] `cargo test --bin aur-pkgbuilder` passes
- [ ] `./dev/scripts/security-check.sh` clean (skips gracefully if audit/deny/gitleaks are not installed)
- [ ] New public items have rustdoc
- [ ] No `unwrap()` / `expect()` outside tests
- [ ] GTK widgets stay on the main thread; long work routed through `runtime::spawn` / `spawn_streaming`
- [ ] Missing external tools degrade gracefully (toast + install hint)
- [ ] README / feature table / Usage section updated if user-visible behavior changed
- [ ] For UI changes: screenshots or a short recording included
- [ ] For config schema changes: `CONFIG_HEADER` / `REGISTRY_HEADER` updated and new fields have `#[serde(default)]`

## Reviewer helpers

```bash
# Local equivalent of the CI lint + security gate
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --bin aur-pkgbuilder
./dev/scripts/security-check.sh
```

## Notes for reviewers

Any additional context, implementation details, or decisions.

## Breaking changes

None (or description if applicable).
