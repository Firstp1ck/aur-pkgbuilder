# pr-update

Compare the current branch against `main` (`git log main..HEAD` +
`git diff --name-status main...HEAD`) and update the matching file in
`dev/PR/` for the active branch.

Integrate missing updates into the existing sections (`Summary`,
`Related issues`, `How to test`, `Checklist`) so the PR reads as one
coherent document. Do **not** use a standalone "additional updates"
append-only section unless explicitly requested.

When updating:
- Keep wording short, specific, and reviewer-focused.
- Include only changes that differ from `main` (final branch state).
- Keep valid entries; rewrite or merge bullets when needed for clarity.
- Remove stale statements that no longer match branch reality (e.g.
  "remaining work" that is now complete).
- Ensure `How to test` reflects current test coverage for the
  implemented scope.
- Keep the `Checklist` in sync with `.github/PULL_REQUEST_TEMPLATE.md`.
