//! Goal item 5 without a DAW: open a real window, attach the shim's editor to
//! it as a DAW would, and save a screenshot of that window.
//!
//! ```text
//! tools/bundle-shim.sh debug
//! TACTUS_SHIM_PRESET=".../Wob Wolfer.nksf" \
//!   cargo run -p tactus-shim --example window -- <out.png> [seconds]
//! ```

use std::ffi::c_void;
use std::path::Path;
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use tactus_plugin_host::vst3::ComPtr;
use tactus_plugin_host::vst3::Steinberg::Vst::IEditControllerTrait;
use tactus_plugin_host::vst3::Steinberg::{IPlugView, IPlugViewTrait, ViewRect, kResultOk};
use tactus_plugin_host::{Module, pump_main_run_loop};
use tactus_shim::FRAME_CID;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let out = args.next().ok_or("usage: window <out.png> [seconds]")?;
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(6);
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    app.finishLaunching();

    let module = Module::load(Path::new("target/debug/Tactus Shim.vst3"))?;
    let mut shim = module.instantiate(&FRAME_CID.map(|b| b as u8))?;
    shim.start_audio(48_000.0, 4096)?;
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(2) {
        let _ = shim.process_block(512);
        pump_main_run_loop(Duration::from_millis(10));
    }

    let rect = NSRect::new(NSPoint::new(120.0, 120.0), NSSize::new(800.0, 500.0));
    let style =
        NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable;
    // SAFETY: standard window creation on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            rect,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: we keep the window alive until the end of main.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Tactus Shim"));
    let content = window.contentView().ok_or("no content view")?;

    // SAFETY: the VST3 editor protocol, on the main thread.
    let view = unsafe {
        let raw = shim.edit_controller().createView(c"editor".as_ptr());
        ComPtr::<IPlugView>::from_raw(raw).ok_or("the shim returned no editor")?
    };
    let parent = objc2::rc::Retained::as_ptr(&content) as *mut c_void;
    let attached = unsafe { view.attached(parent, c"NSView".as_ptr()) };
    println!("attached: {}", attached == kResultOk);
    let mut r = ViewRect {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if unsafe { view.getSize(&mut r) } == kResultOk && r.right > r.left {
        println!("editor size: {}x{}", r.right - r.left, r.bottom - r.top);
        window.setContentSize(NSSize::new(
            f64::from(r.right - r.left),
            f64::from(r.bottom - r.top),
        ));
    }
    window.makeKeyAndOrderFront(None);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(seconds) {
        let _ = shim.process_block(512);
        pump_main_run_loop(Duration::from_millis(10));
    }
    let id = window.windowNumber();
    let shoot = |path: &str| -> Result<(), Box<dyn std::error::Error>> {
        let status = std::process::Command::new("screencapture")
            .args(["-x", "-o", &format!("-l{id}"), path])
            .status()?;
        println!("screenshot: {path} ({status})");
        Ok(())
    };
    shoot(&out)?;

    // Swap the instrument while the window is open, through the same path a
    // DAW's project load and the strip's button take.
    if let Some(other) = std::env::var_os("SWAP_TO") {
        let other = std::path::PathBuf::from(other);
        let preset = tactus_nks::Preset::read(&other)?;
        let saved = tactus_shim::state::SavedState {
            preset: Some(other.clone()),
            bundle: "/Library/Audio/Plug-Ins/VST3/Massive X.vst3".into(),
            cid: tactus_plugin_host::class_id(preset.plugin.vst3_uid.ok_or("no VST3 id")?),
            plugin_state: preset.plugin_state.clone(),
        };
        shim.load_state(&saved.to_bytes())?;
        println!("swapped to {} with the window open", preset.info.name);
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(seconds) {
            let _ = shim.process_block(512);
            pump_main_run_loop(Duration::from_millis(10));
        }
        shoot(&out.replace(".png", "-swapped.png"))?;
    }
    // SAFETY: detach before the view and window go.
    unsafe { view.removed() };
    drop(view);
    window.close();
    Ok(())
}
