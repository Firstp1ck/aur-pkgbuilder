//! Process privilege checks shared by UI and workflow (e.g. before `makepkg`).

/// What: Returns `true` when the current Unix effective user is root (UID 0).
///
/// Inputs:
/// - None.
///
/// Output:
/// - `true` when running as root; always `false` on non-Unix targets.
///
/// Details:
/// - Used by the Build tab and by workflow paths that invoke `makepkg` (validate,
///   register). Keep behavior aligned with the Arch expectation that `makepkg` must
///   not run as root.
pub fn nix_is_root() -> bool {
    nix_is_root_inner()
}

#[cfg(unix)]
fn nix_is_root_inner() -> bool {
    // SAFETY: `getuid` is always safe to call.
    unsafe { libc_getuid() == 0 }
}

#[cfg(not(unix))]
const fn nix_is_root_inner() -> bool {
    false
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}
