//! PKGBUILD freshness banner shared by the Version page and the editor.

use adw::Banner;

use crate::workflow::package::{self, PackageDef};

/// Shows or hides `banner` from [`package::pkgbuild_stale_message`].
pub fn banner_set_pkgbuild_stale(banner: &Banner, pkg: &PackageDef) {
    match package::pkgbuild_stale_message(
        pkg.pkgbuild_refreshed_at_unix,
        package::pkgbuild_refresh_clock_now(),
    ) {
        Some(msg) => {
            banner.set_title(msg);
            banner.set_revealed(true);
        }
        None => {
            banner.set_revealed(false);
        }
    }
}
