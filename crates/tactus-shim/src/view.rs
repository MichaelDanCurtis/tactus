//! The frame's window: the instrument's own editor, with a thin strip above
//! it holding only the NKS settings.
//!
//! The strip shows the preset and plugin and has one button, **Choose
//! preset…**, which loads any NKS preset (and so any VST3 instrument that has
//! presets) into the frame. Nothing else lives here.
//!
//! The instrument's view is a child of ours. Its resize requests reach the
//! DAW through our own `IPlugFrame`, with the strip's height added. When the
//! instrument is replaced while the window is open, its view is detached
//! *before* the old instrument shuts down, then a new one is attached.

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSButton, NSModalResponseOK, NSOpenPanel, NSTextField, NSView};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
use tactus_plugin_host::vst3::Steinberg::Vst::IEditController;
use tactus_plugin_host::vst3::Steinberg::Vst::IEditControllerTrait;
use tactus_plugin_host::vst3::Steinberg::*;
use tactus_plugin_host::vst3::{Class, ComPtr, ComRef, ComWrapper};

use crate::frame::Shared;

/// The strip's height, in points.
pub const STRIP_H: i32 = 30;
/// The window's size when there is no instrument to show.
const EMPTY: (i32, i32) = (520, 80);

/// A value only touched on the main thread.
pub(crate) struct MainOnly<T>(T);

// SAFETY: every access goes through `get`, which needs a MainThreadMarker.
unsafe impl<T> Send for MainOnly<T> {}
unsafe impl<T> Sync for MainOnly<T> {}

impl<T> MainOnly<T> {
    fn get(&self, _mtm: MainThreadMarker) -> &T {
        &self.0
    }
}

/// The open editor, shared between the view and the frame (which must detach
/// the instrument's view before replacing the instrument).
pub(crate) struct Editor {
    inner: Mutex<Option<ComPtr<IPlugView>>>,
    container: MainOnly<Retained<NSView>>,
    host: MainOnly<Retained<NSView>>,
    strip: MainOnly<Retained<NSView>>,
    label: MainOnly<Retained<NSTextField>>,
    button: MainOnly<Retained<NSButton>>,
    _target: MainOnly<Retained<StripTarget>>,
    daw_frame: Mutex<Option<ComPtr<IPlugFrame>>>,
    /// Our own IPlugView, for resize requests to the DAW. Not owned.
    outer: AtomicPtr<IPlugView>,
    proxy: ComWrapper<InnerFrame>,
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> NSRect {
    NSRect::new(
        NSPoint::new(f64::from(x), f64::from(y)),
        NSSize::new(f64::from(w), f64::from(h)),
    )
}

impl Editor {
    /// The instrument view's size, or the empty size.
    fn inner_size(&self) -> (i32, i32) {
        let inner = self.inner.lock().ok().and_then(|g| g.clone());
        let mut r = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        match inner {
            // SAFETY: plain query on a live view.
            Some(v) if unsafe { v.getSize(&mut r) } == kResultOk && r.right > r.left => {
                (r.right - r.left, r.bottom - r.top)
            }
            _ => EMPTY,
        }
    }

    /// Lay out for an inner size of `w` x `h`: the instrument at the bottom,
    /// the strip above it (AppKit's origin is bottom-left).
    fn layout(&self, mtm: MainThreadMarker, w: i32, h: i32) {
        self.container
            .get(mtm)
            .setFrameSize(NSSize::new(f64::from(w), f64::from(h + STRIP_H)));
        self.host.get(mtm).setFrame(rect(0, 0, w, h));
        self.strip.get(mtm).setFrame(rect(0, h, w, STRIP_H));
        self.label
            .get(mtm)
            .setFrame(rect(8, 7, (w - 170).max(80), 18));
        self.button.get(mtm).setFrame(rect(w - 156, 2, 150, 26));
    }

    fn set_label(&self, mtm: MainThreadMarker, text: &str) {
        self.label
            .get(mtm)
            .setStringValue(&NSString::from_str(text));
    }

    /// Detach and release the instrument's view. Call before the instrument
    /// it belongs to shuts down.
    pub(crate) fn detach_inner(&self) {
        if let Some(v) = self.inner.lock().ok().and_then(|mut g| g.take()) {
            // SAFETY: the VST3 editor protocol, on the main thread.
            unsafe {
                v.setFrame(std::ptr::null_mut());
                v.removed();
            }
        }
    }

    /// Create and attach the instrument's view from its controller, then ask
    /// the DAW for the new size.
    pub(crate) fn attach_inner(&self, controller: Option<&ComPtr<IEditController>>, label: &str) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        self.set_label(mtm, label);
        // SAFETY: the VST3 editor protocol, on the main thread.
        let view = controller.and_then(|c| unsafe {
            ComPtr::<IPlugView>::from_raw(c.createView(c"editor".as_ptr()))
        });
        if let Some(v) = &view {
            let parent = Retained::as_ptr(self.host.get(mtm)) as *mut c_void;
            unsafe {
                if let Some(p) = self.proxy.to_com_ptr::<IPlugFrame>() {
                    v.setFrame(p.as_ptr());
                }
                v.attached(parent, c"NSView".as_ptr());
            }
        }
        if let Ok(mut g) = self.inner.lock() {
            *g = view;
        }
        let (w, h) = self.inner_size();
        self.layout(mtm, w, h);
        self.request_size(w, h);
    }

    /// Ask the DAW to make our view fit an inner size of `w` x `h`.
    fn request_size(&self, w: i32, h: i32) {
        let frame = self.daw_frame.lock().ok().and_then(|g| g.clone());
        let outer = self.outer.load(Ordering::Acquire);
        if let Some(f) = frame
            && !outer.is_null()
        {
            let mut r = ViewRect {
                left: 0,
                top: 0,
                right: w,
                bottom: h + STRIP_H,
            };
            // SAFETY: the DAW's frame and our own live view.
            unsafe { f.resizeView(outer, &mut r) };
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.detach_inner();
    }
}

/// The `IPlugFrame` handed to the instrument's view: its resize requests go
/// to the DAW with the strip added.
pub(crate) struct InnerFrame {
    editor: Mutex<std::sync::Weak<Editor>>,
}

impl Class for InnerFrame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for InnerFrame {
    unsafe fn resizeView(&self, _view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
        let Some(r) = (unsafe { new_size.as_ref() }) else {
            return kInvalidArgument;
        };
        if let Some(ed) = self.editor.lock().ok().and_then(|w| w.upgrade()) {
            ed.request_size(r.right - r.left, r.bottom - r.top);
        }
        kResultOk
    }
}

pub(crate) struct TargetIvars {
    shared: std::sync::Weak<Shared>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TactusShimStripTarget_0_1"]
    #[ivars = TargetIvars]
    pub(crate) struct StripTarget;

    impl StripTarget {
        #[unsafe(method(choosePreset:))]
        fn choose_preset(&self, _sender: Option<&AnyObject>) {
            let mtm = self.mtm();
            // Test seam: TACTUS_SHIM_PICK names the file instead of asking,
            // so an automated click can run the real path after the panel.
            if let Some(path) = std::env::var_os("TACTUS_SHIM_PICK") {
                if let Some(shared) = self.ivars().shared.upgrade() {
                    crate::frame::choose_preset(&shared, std::path::Path::new(&path));
                }
                return;
            }
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            #[allow(deprecated)]
            panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&[NSString::from_str("nksf")])));
            if panel.runModal() != NSModalResponseOK {
                return;
            }
            let Some(path) = panel.URL().and_then(|u| u.path()).map(|p| p.to_string()) else {
                return;
            };
            if let Some(shared) = self.ivars().shared.upgrade() {
                crate::frame::choose_preset(&shared, std::path::Path::new(&path));
            }
        }
    }
);

impl StripTarget {
    fn new(mtm: MainThreadMarker, shared: std::sync::Weak<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { shared });
        // SAFETY: NSObject's init.
        unsafe { msg_send![super(this), init] }
    }
}

/// The frame's `IPlugView`.
pub struct FrameView {
    shared: std::sync::Weak<Shared>,
    editor: Mutex<Option<Arc<Editor>>>,
    daw_frame: Mutex<Option<ComPtr<IPlugFrame>>>,
    me: AtomicPtr<IPlugView>,
}

impl Class for FrameView {
    type Interfaces = (IPlugView,);
}

impl FrameView {
    /// A new view for the frame. `attached` builds the AppKit side.
    pub(crate) fn create(shared: std::sync::Weak<Shared>) -> *mut IPlugView {
        let view = ComWrapper::new(FrameView {
            shared,
            editor: Mutex::new(None),
            daw_frame: Mutex::new(None),
            me: AtomicPtr::new(std::ptr::null_mut()),
        });
        match view.to_com_ptr::<IPlugView>() {
            Some(p) => {
                view.me.store(p.as_ptr(), Ordering::Release);
                p.into_raw()
            }
            None => std::ptr::null_mut(),
        }
    }

    fn editor(&self) -> Option<Arc<Editor>> {
        self.editor.lock().ok().and_then(|g| g.clone())
    }

    fn inner(&self) -> Option<ComPtr<IPlugView>> {
        self.editor()
            .and_then(|e| e.inner.lock().ok().and_then(|g| g.clone()))
    }
}

fn is_nsview(t: FIDString) -> bool {
    // SAFETY: the DAW passes a NUL-terminated platform type.
    !t.is_null() && unsafe { CStr::from_ptr(t) } == c"NSView"
}

impl IPlugViewTrait for FrameView {
    unsafe fn isPlatformTypeSupported(&self, t: FIDString) -> tresult {
        if is_nsview(t) {
            kResultOk
        } else {
            kResultFalse
        }
    }

    unsafe fn attached(&self, parent: *mut c_void, t: FIDString) -> tresult {
        let (Some(mtm), true, false) = (MainThreadMarker::new(), is_nsview(t), parent.is_null())
        else {
            return kResultFalse;
        };
        let Some(shared) = self.shared.upgrade() else {
            return kResultFalse;
        };
        // SAFETY: the DAW passes a live NSView.
        let parent: &NSView = unsafe { &*(parent as *const NSView) };
        let container =
            NSView::initWithFrame(NSView::alloc(mtm), rect(0, 0, EMPTY.0, EMPTY.1 + STRIP_H));
        let host = NSView::initWithFrame(NSView::alloc(mtm), rect(0, 0, EMPTY.0, EMPTY.1));
        let strip = NSView::initWithFrame(NSView::alloc(mtm), rect(0, EMPTY.1, EMPTY.0, STRIP_H));
        let label = NSTextField::labelWithString(&NSString::from_str("Tactus Shim"), mtm);
        let target = StripTarget::new(mtm, self.shared.clone());
        // SAFETY: the target outlives the button (both held by the editor).
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Choose preset…"),
                Some(&target),
                Some(sel!(choosePreset:)),
                mtm,
            )
        };
        strip.addSubview(&label);
        strip.addSubview(&button);
        container.addSubview(&host);
        container.addSubview(&strip);
        parent.addSubview(&container);

        let editor = Arc::new_cyclic(|weak| Editor {
            inner: Mutex::new(None),
            container: MainOnly(container),
            host: MainOnly(host),
            strip: MainOnly(strip),
            label: MainOnly(label),
            button: MainOnly(button),
            _target: MainOnly(target),
            daw_frame: Mutex::new(self.daw_frame.lock().ok().and_then(|g| g.clone())),
            outer: AtomicPtr::new(self.me.load(Ordering::Acquire)),
            proxy: ComWrapper::new(InnerFrame {
                editor: Mutex::new(weak.clone()),
            }),
        });
        if let Ok(mut g) = self.editor.lock() {
            *g = Some(Arc::clone(&editor));
        }
        crate::frame::editor_opened(&shared, &editor);
        kResultOk
    }

    unsafe fn removed(&self) -> tresult {
        if let Some(editor) = self.editor.lock().ok().and_then(|mut g| g.take()) {
            editor.detach_inner();
            if let Some(mtm) = MainThreadMarker::new() {
                editor.container.get(mtm).removeFromSuperview();
            }
            if let Some(shared) = self.shared.upgrade() {
                crate::frame::editor_closed(&shared);
            }
        }
        kResultOk
    }

    unsafe fn onWheel(&self, distance: f32) -> tresult {
        self.inner()
            .map_or(kResultFalse, |v| unsafe { v.onWheel(distance) })
    }

    unsafe fn onKeyDown(&self, key: char16, code: int16, mods: int16) -> tresult {
        self.inner()
            .map_or(kResultFalse, |v| unsafe { v.onKeyDown(key, code, mods) })
    }

    unsafe fn onKeyUp(&self, key: char16, code: int16, mods: int16) -> tresult {
        self.inner()
            .map_or(kResultFalse, |v| unsafe { v.onKeyUp(key, code, mods) })
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        let Some(out) = (unsafe { size.as_mut() }) else {
            return kInvalidArgument;
        };
        let (w, h) = match self.editor() {
            Some(e) => e.inner_size(),
            None => {
                // Before attachment: ask the instrument directly if we can.
                let mut r = ViewRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };
                match self
                    .shared
                    .upgrade()
                    .and_then(|s| crate::frame::preview_size(&s, &mut r))
                {
                    Some(()) if r.right > r.left => (r.right - r.left, r.bottom - r.top),
                    _ => EMPTY,
                }
            }
        };
        *out = ViewRect {
            left: 0,
            top: 0,
            right: w,
            bottom: h + STRIP_H,
        };
        kResultOk
    }

    unsafe fn onSize(&self, new_size: *mut ViewRect) -> tresult {
        let (Some(r), Some(editor), Some(mtm)) = (
            unsafe { new_size.as_ref() },
            self.editor(),
            MainThreadMarker::new(),
        ) else {
            return kResultOk;
        };
        let (w, h) = (r.right - r.left, (r.bottom - r.top - STRIP_H).max(0));
        editor.layout(mtm, w, h);
        if let Some(v) = self.inner() {
            let mut inner = ViewRect {
                left: 0,
                top: 0,
                right: w,
                bottom: h,
            };
            unsafe { v.onSize(&mut inner) };
        }
        kResultOk
    }

    unsafe fn onFocus(&self, state: TBool) -> tresult {
        self.inner()
            .map_or(kResultFalse, |v| unsafe { v.onFocus(state) })
    }

    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> tresult {
        // SAFETY: the DAW's frame; keep our own reference.
        let f = unsafe { ComRef::from_raw(frame) }.map(|f| f.to_com_ptr());
        if let Some(e) = self.editor()
            && let Ok(mut g) = e.daw_frame.lock()
        {
            *g = f.clone();
        }
        if let Ok(mut g) = self.daw_frame.lock() {
            *g = f;
        }
        kResultOk
    }

    unsafe fn canResize(&self) -> tresult {
        self.inner()
            .map_or(kResultFalse, |v| unsafe { v.canResize() })
    }

    unsafe fn checkSizeConstraint(&self, rect: *mut ViewRect) -> tresult {
        let (Some(r), Some(v)) = (unsafe { rect.as_mut() }, self.inner()) else {
            return kResultFalse;
        };
        let mut inner = ViewRect {
            bottom: r.bottom - STRIP_H,
            ..*r
        };
        let code = unsafe { v.checkSizeConstraint(&mut inner) };
        *r = ViewRect {
            bottom: inner.bottom + STRIP_H,
            ..inner
        };
        code
    }
}
