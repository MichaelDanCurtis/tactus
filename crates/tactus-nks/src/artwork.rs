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

/// A product's knob-arc colour, `NKS2_control_color`, as `[r, g, b]`.
///
/// Read from NI's colour database: the product's own copy first, then the
/// shared one under `Komplete Kontrol`. The database is opened read-only, and
/// the product name is bound as a query parameter, never spliced into SQL.
#[cfg(feature = "resources")]
pub fn control_color(root: &Path, product: &str) -> Option<[u8; 3]> {
    use rusqlite::{Connection, OpenFlags};

    if !safe_component(product) {
        return None;
    }
    let db = |folder: &str| {
        root.join(folder)
            .join("PAResources/_DatabaseResources/color/color.db")
    };
    [db(product), db("Komplete Kontrol")]
        .iter()
        .filter(|p| p.is_file())
        .find_map(|path| {
            let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
            let hex: String = conn
                .query_row(
                    "SELECT Value FROM color WHERE Location = ?1 \
                     AND Name = 'NKS2_control_color' LIMIT 1",
                    [product],
                    |row| row.get(0),
                )
                .ok()?;
            parse_hex_rgb(&hex)
        })
}

/// `"FC6949"` → `[0xfc, 0x69, 0x49]`. Exactly six hex digits.
pub fn parse_hex_rgb(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.trim();
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| {
        hex.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
    };
    Some([byte(0)?, byte(2)?, byte(4)?])
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

    #[test]
    fn hex_colours_parse_exactly() {
        assert_eq!(parse_hex_rgb("FC6949"), Some([0xfc, 0x69, 0x49]));
        assert_eq!(parse_hex_rgb("e17600"), Some([0xe1, 0x76, 0x00]));
        for bad in ["", "FC694", "FC69490", "GG0000", "#FC6949", "FC 949"] {
            assert_eq!(parse_hex_rgb(bad), None, "{bad:?}");
        }
    }

    #[cfg(feature = "resources")]
    #[test]
    fn the_products_own_colour_wins_and_names_are_bound_not_spliced() {
        let root = scratch("color");
        let make = |folder: &str, rows: &[(&str, &str)]| {
            let dir = root
                .join(folder)
                .join("PAResources/_DatabaseResources/color");
            std::fs::create_dir_all(&dir).unwrap();
            let conn = rusqlite::Connection::open(dir.join("color.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE color (Location text, Name text, Value text, seqidx integer)",
            )
            .unwrap();
            for (loc, val) in rows {
                conn.execute(
                    "INSERT INTO color VALUES (?1, 'NKS2_control_color', ?2, 0)",
                    [loc, val],
                )
                .unwrap();
            }
        };
        make(
            "Komplete Kontrol",
            &[("Super 8 R2", "000000"), ("FM8", "EBE9EA")],
        );
        make("Super 8 R2", &[("Super 8 R2", "FC6949")]);
        assert_eq!(control_color(&root, "Super 8 R2"), Some([0xfc, 0x69, 0x49]));
        assert_eq!(control_color(&root, "FM8"), Some([0xeb, 0xe9, 0xea]));
        assert_eq!(control_color(&root, "Nothing"), None);
        assert_eq!(control_color(&root, "x' OR '1'='1"), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
