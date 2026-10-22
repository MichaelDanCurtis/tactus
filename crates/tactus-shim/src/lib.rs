//! The shim: a thin VST3 frame that a DAW loads, which hosts one instrument
//! and puts it on a Kontrol keyboard.
//!
//! The DAW sees an ordinary instrument whose parameters, MIDI mapping and
//! window are the hosted instrument's own. Audio and MIDI pass straight
//! through. The keyboard shows the instrument's NKS pages, banner and arc
//! colour; its knobs, page and preset buttons work; held keys light up.
//!
//! See `docs/shim.md`.

#![deny(missing_docs)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod frame;
mod instrument;
mod keyboard;
pub mod state;
mod view;

use std::ffi::{CString, c_char, c_void};

use tactus_plugin_host::vst3::Steinberg::*;
use tactus_plugin_host::vst3::{Class, ComWrapper, uid};

pub use frame::Frame;

/// The frame's VST3 class id. Fixed forever: DAW projects refer to it.
pub const FRAME_CID: TUID = uid(0x7A637573, 0x53484D31, 0x9F1C4E2B, 0x6D0A3B71);

/// The frame's display name.
pub const NAME: &str = "Tactus Shim";
const VENDOR: &str = "tactus";

fn copy(src: &str, dst: &mut [c_char]) {
    let c = CString::new(src).unwrap_or_default();
    let bytes = c.as_bytes_with_nul();
    for (d, s) in dst.iter_mut().zip(bytes) {
        *d = *s as c_char;
    }
    if bytes.len() > dst.len()
        && let Some(last) = dst.last_mut()
    {
        *last = 0;
    }
}

/// The module's factory: one class, the frame.
pub struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        copy(VENDOR, &mut info.vendor);
        copy("", &mut info.url);
        copy("", &mut info.email);
        info.flags = PFactoryInfo_::FactoryFlags_::kUnicode as int32;
        kResultOk
    }

    unsafe fn countClasses(&self) -> int32 {
        1
    }

    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 {
            return kInvalidArgument;
        }
        info.cid = FRAME_CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy("Audio Module Class", &mut info.category);
        copy(NAME, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if cid.is_null() || iid.is_null() || obj.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: cid points at a 16-byte TUID.
        if unsafe { *(cid as *const TUID) } != FRAME_CID {
            return kInvalidArgument;
        }
        let Some(instance) = ComWrapper::new(Frame::default()).to_com_ptr::<FUnknown>() else {
            return kInternalError;
        };
        let ptr = instance.as_ptr();
        // SAFETY: standard COM query on the object just made.
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid as *const TUID, obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 {
            return kInvalidArgument;
        }
        info.cid = FRAME_CID;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy("Audio Module Class", &mut info.category);
        copy(NAME, &mut info.name);
        info.classFlags = 0;
        copy("Instrument", &mut info.subCategories);
        copy(VENDOR, &mut info.vendor);
        copy(env!("CARGO_PKG_VERSION"), &mut info.version);
        copy("VST 3.7.0", &mut info.sdkVersion);
        kResultOk
    }
}

/// VST3 module entry: the factory.
#[unsafe(no_mangle)]
pub extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |f| f.into_raw())
}

/// macOS module entry.
#[unsafe(no_mangle)]
pub extern "C" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

/// macOS module exit.
#[unsafe(no_mangle)]
pub extern "C" fn bundleExit() -> bool {
    true
}
