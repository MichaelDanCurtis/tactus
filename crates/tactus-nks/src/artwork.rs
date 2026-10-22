//! Product artwork on disk: the banner the keyboard shows above the knobs.
//!
//! NI products ship their images under `PAResources/image/<product>/`, inside
//! the product's own folder or a shared one. The MK3's banner is
//! `NKS2_hardware_banner.webp`, 1280x212. 1,616 of them were found on a
//! development machine, all that size.

use std::path::{Path, PathBuf};

/// Where NI products install their support files on macOS.
pub const MAC_RESOURCE_ROOT: &str = "/Library/Application Support/Native Instruments";

/// The MK3 header banner's file name.
pub const HARDWARE_BANNER: &str = "NKS2_hardware_banner.webp";

/// Whether a product name is safe to use as one path component.
///
/// It comes from preset metadata, which is untrusted: a name such as
/// `../../etc` must not walk out of the resource folder.
fn safe_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// Find a product's hardware banner under `root`.
///
/// Looks in `root/<product>/` first, then in every folder up to two levels
/// below `root` (the shared copies live in, for example,
/// `Hardware/Hardware Connection Service`). `product` is normally the first
/// entry of a preset's bank chain.
pub fn hardware_banner(root: &Path, product: &str) -> Option<PathBuf> {
    if !safe_component(product) {
        return None;
    }
    let banner_in = |dir: &Path| {
        let p = dir
            .join("PAResources")
            .join("image")
            .join(product)
            .join(HARDWARE_BANNER);
        p.is_file().then_some(p)
    };
    if let Some(p) = banner_in(&root.join(product)) {
        return Some(p);
    }
    let subdirs = |dir: &Path| -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    };
    for first in subdirs(root) {
        if let Some(p) = banner_in(&first) {
            return Some(p);
        }
        for second in subdirs(&first) {
            if let Some(p) = banner_in(&second) {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tactus-nks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn place(root: &Path, under: &str, product: &str) -> PathBuf {
        let dir = root.join(under).join("PAResources/image").join(product);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(HARDWARE_BANNER);
        std::fs::write(&p, b"RIFF\0\0\0\0WEBP").unwrap();
        p
    }

    #[test]
    fn the_products_own_folder_wins() {
        let root = scratch("own");
        let shared = place(&root, "Hardware/Hardware Connection Service", "Super 8 R2");
        assert_eq!(hardware_banner(&root, "Super 8 R2"), Some(shared));
        let own = place(&root, "Super 8 R2", "Super 8 R2");
        assert_eq!(hardware_banner(&root, "Super 8 R2"), Some(own));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_banner_is_none() {
        let root = scratch("none");
        assert_eq!(hardware_banner(&root, "Nothing Here"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_product_name_cannot_leave_the_resource_folder() {
        let root = scratch("escape");
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", "nul\0"] {
            assert_eq!(hardware_banner(&root, bad), None, "{bad:?}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
