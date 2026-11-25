//! The writing example from `docs/nks-plugin-guide.md`, compiled.
//!
//! ```text
//! cargo run -p tactus-nks --example write_preset -- <out.nksf>
//! ```
//!
//! The plugin state here is a placeholder; a real preset carries what the
//! plugin's `IComponent::getState` writes.

use tactus_nks::{Info, NksError, Page, PluginId, Preset, Slot};

fn main() -> Result<(), NksError> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Warm Pad.nksf".into());
    let your_plugin_state = vec![0u8; 16];

    let knob = |id, name: &str, section: Option<&str>| {
        Some(Slot {
            id,
            name: name.into(),
            section: section.map(Into::into),
            autoname: false,
            vflag: false,
        })
    };
    let preset = Preset {
        info: Info {
            name: "Warm Pad".into(),
            vendor: "Your Company".into(),
            author: "You".into(),
            bankchain: vec!["Your Synth".into(), "Factory".into(), "".into()],
            types: vec![vec!["Pad".into()]],
            device_type: "INST".into(),
            ..Default::default()
        },
        plugin: PluginId {
            vst_magic: 0,
            vst3_uid: Some([0x1234_5678, 0x9ABC_DEF0, 0x1122_3344, 0x5566_7788]),
            name: Some("Your Synth".into()),
            vendor: Some("Your Company".into()),
        },
        pages: vec![Page {
            slots: vec![
                knob(10, "Cutoff", Some("Filter")),
                knob(11, "Reso", None),
                None,
                None,
                knob(20, "Attack", Some("Amp")),
                knob(21, "Decay", None),
                knob(22, "Sustain", None),
                knob(23, "Release", None),
            ],
            ..Default::default()
        }],
        plugin_state: your_plugin_state,
        ..Default::default()
    };
    preset.write(&out)?;

    let back = Preset::read(&out)?;
    println!(
        "{out}: {} — {} page(s), sections {:?}",
        back.info.name,
        back.pages.len(),
        back.pages.first().map(|p| p
            .sections()
            .iter()
            .map(|s| s.name.to_owned())
            .collect::<Vec<_>>())
    );
    Ok(())
}
