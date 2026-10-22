//! The hosted instrument: which plugin, which preset, and its saved state.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tactus_nks::Preset;
use tactus_plugin_host::{Module, Plugin, class_id};

use crate::state::SavedState;

/// Where the frame looks for its default preset until the settings strip
/// exists: `~/Library/Application Support/tactus/shim.conf`, a line
/// `preset=/path/to/file.nksf`. `TACTUS_SHIM_PRESET` overrides it.
pub fn default_preset() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TACTUS_SHIM_PRESET") {
        return Some(PathBuf::from(p));
    }
    let conf = support_dir()?.join("shim.conf");
    let text = std::fs::read_to_string(conf).ok()?;
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("preset="))
        .map(|p| PathBuf::from(p.trim()))
        .next()
}

/// `~/Library/Application Support/tactus`.
pub fn support_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(Path::new(&home).join("Library/Application Support/tactus"))
}

/// The bundle for a plugin name: the user's VST3 folder first, then the
/// system's.
fn bundle_for(name: &str) -> Option<PathBuf> {
    if name.contains(['/', '\\', '\0']) || name.is_empty() || name == ".." {
        return None;
    }
    let file = format!("{name}.vst3");
    let user = std::env::var_os("HOME").map(|h| {
        Path::new(&h)
            .join("Library/Audio/Plug-Ins/VST3")
            .join(&file)
    });
    let system = Path::new("/Library/Audio/Plug-Ins/VST3").join(&file);
    user.into_iter().chain([system]).find(|p| p.is_dir())
}

/// A loaded instrument.
pub struct Instrument {
    /// The running plugin.
    pub plugin: Plugin,
    /// The NKS preset it was loaded from, for its pages, banner and colour.
    pub preset: Option<Preset>,
    /// That preset's path.
    pub preset_path: Option<PathBuf>,
    /// The plugin's bundle.
    pub bundle: PathBuf,
    /// The plugin's class id.
    pub cid: [u8; 16],
    /// State to hand the plugin once its engine runs; some plugins apply a
    /// state only then.
    pub pending_state: Option<Vec<u8>>,
}

impl Instrument {
    fn open(bundle: &Path, cid: [u8; 16]) -> Result<Plugin, String> {
        let module: Arc<Module> = Module::load(bundle).map_err(|e| e.to_string())?;
        module.instantiate(&cid).map_err(|e| e.to_string())
    }

    /// Load the plugin an NKS preset names, with the preset's state.
    pub fn from_preset(path: &Path) -> Result<Self, String> {
        let preset = Preset::read(path).map_err(|e| e.to_string())?;
        let uid = preset
            .plugin
            .vst3_uid
            .ok_or("the preset names no VST3 class")?;
        let name = preset
            .plugin
            .name
            .clone()
            .ok_or("the preset does not name its plugin")?;
        let bundle = bundle_for(&name).ok_or_else(|| format!("no {name}.vst3 installed"))?;
        let cid = class_id(uid);
        let plugin = Self::open(&bundle, cid)?;
        let state = preset.plugin_state.clone();
        let _ = plugin.load_state(&state);
        Ok(Self {
            plugin,
            preset: Some(preset),
            preset_path: Some(path.to_owned()),
            bundle,
            cid,
            pending_state: Some(state),
        })
    }

    /// Restore what a DAW project saved.
    pub fn from_saved(saved: &SavedState) -> Result<Self, String> {
        let plugin = Self::open(&saved.bundle, saved.cid)?;
        let _ = plugin.load_state(&saved.plugin_state);
        let preset = saved.preset.as_ref().and_then(|p| Preset::read(p).ok());
        Ok(Self {
            plugin,
            preset,
            preset_path: saved.preset.clone(),
            bundle: saved.bundle.clone(),
            cid: saved.cid,
            pending_state: Some(saved.plugin_state.clone()),
        })
    }

    /// What to save in the DAW project now.
    pub fn saved(&self) -> SavedState {
        SavedState {
            preset: self.preset_path.clone(),
            bundle: self.bundle.clone(),
            cid: self.cid,
            plugin_state: self
                .pending_state
                .clone()
                .or_else(|| self.plugin.save_state().ok())
                .unwrap_or_default(),
        }
    }

    /// Switch to another preset of the same plugin, keeping the plugin.
    pub fn switch_preset(&mut self, path: &Path) -> Result<(), String> {
        let preset = Preset::read(path).map_err(|e| e.to_string())?;
        if preset.plugin.vst3_uid.map(class_id) != Some(self.cid) {
            return Err("that preset is for another plugin".into());
        }
        self.plugin
            .load_state(&preset.plugin_state)
            .map_err(|e| e.to_string())?;
        self.preset = Some(preset);
        self.preset_path = Some(path.to_owned());
        self.pending_state = None;
        Ok(())
    }

    /// The `.nksf` files beside the current preset, sorted.
    pub fn folder(&self) -> Vec<PathBuf> {
        let Some(dir) = self.preset_path.as_ref().and_then(|p| p.parent()) else {
            return Vec::new();
        };
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "nksf"))
                    .collect()
            })
            .unwrap_or_default();
        all.sort();
        all
    }
}
