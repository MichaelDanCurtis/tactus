//! Rewrite presets with [`Preset::to_bytes`] and check them.
//!
//! ```text
//! cargo run -p tactus-nks --example roundtrip -- <preset.nksf>...
//! ```
//!
//! For each file: does the rewrite read back to the same preset, and is it
//! byte-for-byte the original? Nothing is written to disk.

use std::process::ExitCode;

use tactus_nks::Preset;

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let (mut same, mut identical, mut failed) = (0, 0, 0);
    let mut first_diff = None;
    for path in &paths {
        let Ok(original) = std::fs::read(path) else {
            failed += 1;
            continue;
        };
        let Ok(p) = Preset::parse(&original) else {
            failed += 1;
            continue;
        };
        let written = p.to_bytes();
        if let Some(out) = std::env::var_os("ROUNDTRIP_DUMP") {
            let _ = std::fs::write(out, &written);
        }
        match Preset::parse(&written) {
            Ok(again) if again == p => same += 1,
            _ => {
                failed += 1;
                eprintln!("{path}: rewrite does not read back the same");
            }
        }
        if written == original {
            identical += 1;
        } else if first_diff.is_none() {
            let at = written.iter().zip(&original).position(|(a, b)| a != b);
            first_diff = Some((path.clone(), original.len(), written.len(), at));
        }
    }
    println!(
        "{} files: {same} read back the same, {identical} byte-identical, {failed} failed",
        paths.len()
    );
    if let Some((path, a, b, at)) = first_diff {
        println!(
            "first difference: {path}: original {a} bytes, rewrite {b}, first differing byte {at:?}"
        );
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
