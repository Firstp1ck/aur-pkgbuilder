# release-new

Create a new release file under `Release-docs/RELEASE_v<version>.md` for
the given version and automatically generate the release notes — check
changes since the last release. Keep the release file user-friendly,
short, concise, and clear.

Once the file exists and is committed:

1. Bump `version` in `Cargo.toml`.
2. Commit both files with `chore: release v<version>`.
3. Tag the commit: `git tag -a v<version> -m "Release v<version>"`.
4. Push the tag: `git push origin v<version>`.

The `release.yml` workflow picks up the tag, builds x86_64 and aarch64
binaries, attaches them plus `SHA256SUMS` to a GitHub release, and the
maintainer then bumps `PKGBUILD-bin` and pushes to the AUR using
`dev/scripts/aur-push.sh`.
