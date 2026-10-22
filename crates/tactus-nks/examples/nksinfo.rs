//! Print what an NKS preset tells a host: metadata, plugin id, knob pages.
//!
//! ```text
//! cargo run -p tactus-nks --example nksinfo -- <preset.nksf>...
//! ```
//!
//! With more than one file it prints a one-line summary each and a tally of
//! failures, which makes it a quick check of the reader over a whole library.

use std::process::ExitCode;

use tactus_nks::Preset;

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: nksinfo <preset.nksf>...");
        return ExitCode::FAILURE;
    }
    let mut failed = 0;
    for path in &paths {
        match Preset::read(path) {
            Ok(p) if paths.len() == 1 => detail(&p),
            Ok(p) => println!(
                "{:<32} {:<12} {} pages, {} knobs",
                p.info.name,
                p.plugin
                    .vst_magic_fourcc()
                    .or(p.plugin.name.clone())
                    .unwrap_or_default(),
                p.pages.len(),
                p.pages
                    .iter()
                    .flat_map(|pg| pg.slots.iter().flatten())
                    .count()
            ),
            Err(e) => {
                failed += 1;
                eprintln!("{path}: {e}");
            }
        }
    }
    if paths.len() > 1 {
        println!("{} read, {failed} failed", paths.len() - failed);
    }
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn detail(p: &Preset) {
    println!("name     {}", p.info.name);
    println!("vendor   {}", p.info.vendor);
    println!("author   {}", p.info.author);
    println!("bank     {}", p.info.bankchain.join(" / "));
    println!("types    {:?}", p.info.types);
    println!(
        "plugin   {} (VST {:#010x}{})",
        p.plugin.name.as_deref().unwrap_or("-"),
        p.plugin.vst_magic,
        p.plugin
            .vst_magic_fourcc()
            .map(|c| format!(" '{c}'"))
            .unwrap_or_default()
    );
    println!("state    {} bytes, opaque", p.state_len);
    for (i, page) in p.pages.iter().enumerate() {
        if page.is_empty() {
            continue;
        }
        println!("page {}", i + 1);
        for section in page.sections() {
            let knobs: Vec<String> = section
                .slots
                .iter()
                .map(|s| s.map_or("-".into(), |s| format!("{}#{}", s.name, s.id)))
                .collect();
            println!("  [{}] {}", section.name, knobs.join(", "));
        }
    }
}
