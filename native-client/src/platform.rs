use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub(crate) struct ExternalDropPosition {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) physical: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum ExternalImageDrop {
    Paths(Vec<PathBuf>, Option<ExternalDropPosition>),
    #[cfg(windows)]
    Text(String, Option<ExternalDropPosition>),
}

static EXTERNAL_IMAGE_DROPS: OnceLock<Mutex<Vec<ExternalImageDrop>>> = OnceLock::new();

#[cfg(target_os = "macos")]
static PENDING_MACOS_FILE_DRAG: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn external_image_drops() -> &'static Mutex<Vec<ExternalImageDrop>> {
    EXTERNAL_IMAGE_DROPS.get_or_init(|| Mutex::new(Vec::new()))
}

fn queue_external_image_drop(drop: ExternalImageDrop) {
    if let Ok(mut drops) = external_image_drops().lock() {
        drops.push(drop);
    }
}

pub(crate) fn take_external_image_drops() -> Vec<ExternalImageDrop> {
    external_image_drops()
        .lock()
        .map(|mut drops| std::mem::take(&mut *drops))
        .unwrap_or_default()
}

#[cfg(target_os = "macos")]
pub(crate) fn queue_macos_file_drag(path: PathBuf) -> bool {
    if !path.is_file() {
        return false;
    }
    PENDING_MACOS_FILE_DRAG
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map(|mut pending| {
            *pending = Some(path);
            true
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn take_macos_file_drag() -> Option<PathBuf> {
    PENDING_MACOS_FILE_DRAG
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|mut pending| pending.take())
}

#[cfg(windows)]
pub(crate) fn install_external_image_drop_target(window: &slint::Window) -> bool {
    windows_drop_target::install(window)
}

#[cfg(target_os = "macos")]
pub(crate) fn install_external_image_drop_target(window: &slint::Window) -> bool {
    macos_drop_target::install(window)
}

#[cfg(all(not(windows), not(target_os = "macos")))]
pub(crate) fn install_external_image_drop_target(_window: &slint::Window) -> bool {
    true
}

#[cfg(target_os = "macos")]
pub(crate) fn schedule_application_icon_install() {
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        if let Err(error) = install_macos_app_icon() {
            eprintln!("failed to install macOS application icon: {error:#}");
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn schedule_application_icon_install() {}

#[cfg(target_os = "macos")]
fn install_macos_app_icon() -> anyhow::Result<()> {
    use anyhow::{anyhow, Context};
    use objc2::{AllocAnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new()
        .ok_or_else(|| anyhow!("macOS application icon must be installed on the main thread"))?;
    let icon_data = NSData::with_bytes(include_bytes!("../assets/app-icon-macos.png"));
    let icon = NSImage::initWithData(NSImage::alloc(), &icon_data)
        .context("decode embedded macOS application icon")?;
    let application = NSApplication::sharedApplication(main_thread);

    // SAFETY: AppKit retains the supplied NSImage and this runs on the main thread.
    unsafe { application.setApplicationIconImage(Some(&icon)) };
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // winit 0.30 registers Finder drops with NSFilenamesPboardType.
mod macos_drop_target {
    use super::{
        queue_external_image_drop, take_macos_file_drag, ExternalDropPosition, ExternalImageDrop,
    };
    use objc2::{
        ffi, msg_send,
        rc::Retained,
        runtime::{AnyClass, AnyObject, Bool, Imp, Sel},
        sel,
    };
    use objc2_app_kit::{NSEvent, NSFilenamesPboardType, NSPasteboard, NSView};
    use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static ORIGINAL_MOUSE_DRAGGED: OnceLock<Imp> = OnceLock::new();
    static ORIGINAL_MOUSE_UP: OnceLock<Imp> = OnceLock::new();

    pub(super) fn install(window: &slint::Window) -> bool {
        let window_handle = window.window_handle();
        let Ok(window_handle) = window_handle.window_handle() else {
            return false;
        };
        let RawWindowHandle::AppKit(handle) = window_handle.as_raw() else {
            return false;
        };

        // SAFETY: Slint owns the AppKit view for at least as long as this window handle.
        let Some(view) = (unsafe { handle.ns_view.as_ptr().cast::<AnyObject>().as_ref() }) else {
            return false;
        };
        // SAFETY: `window` is a standard AppKit accessor. The returned object remains
        // owned by Slint/winit while the application window exists.
        let native_window: *mut AnyObject = unsafe { msg_send![view, window] };
        let Some(native_window) = (unsafe { native_window.as_ref() }) else {
            return false;
        };
        if !install_file_drag_method(view.class()) || !install_drop_methods(native_window.class()) {
            return false;
        }

        let dragged_types = NSArray::from_slice(&[
            // SAFETY: AppKit exposes this as a process-lifetime NSString constant.
            unsafe { NSFilenamesPboardType },
        ]);
        // SAFETY: `native_window` is an NSWindow and the array contains pasteboard types.
        unsafe {
            let _: () = msg_send![native_window, registerForDraggedTypes: &*dragged_types];
        }
        true
    }

    fn install_file_drag_method(class: &'static AnyClass) -> bool {
        if ORIGINAL_MOUSE_DRAGGED.get().is_some() && ORIGINAL_MOUSE_UP.get().is_some() {
            return true;
        }
        let drag_selector = sel!(mouseDragged:);
        let up_selector = sel!(mouseUp:);
        // SAFETY: Both selectors are inherited by every NSView. Their implementations
        // and type encodings remain valid for winit's concrete view subclass.
        let drag_method = unsafe { ffi::class_getInstanceMethod(class, drag_selector) };
        let up_method = unsafe { ffi::class_getInstanceMethod(class, up_selector) };
        if drag_method.is_null() || up_method.is_null() {
            return false;
        }
        let Some(original_dragged) = (unsafe { ffi::method_getImplementation(drag_method) }) else {
            return false;
        };
        let Some(original_up) = (unsafe { ffi::method_getImplementation(up_method) }) else {
            return false;
        };
        let drag_types = unsafe { ffi::method_getTypeEncoding(drag_method) };
        let up_types = unsafe { ffi::method_getTypeEncoding(up_method) };
        if drag_types.is_null()
            || up_types.is_null()
            || ORIGINAL_MOUSE_DRAGGED.set(original_dragged).is_err()
            || ORIGINAL_MOUSE_UP.set(original_up).is_err()
        {
            return false;
        }
        let mouse_dragged: extern "C-unwind" fn(_, _, _) = mouse_dragged;
        let mouse_up: extern "C-unwind" fn(_, _, _) = mouse_up;
        // SAFETY: The override has the same ABI and encoding as NSResponder's
        // pointer methods and is installed only on winit's concrete NSView class.
        unsafe {
            ffi::class_replaceMethod(
                class as *const AnyClass as *mut AnyClass,
                drag_selector,
                std::mem::transmute::<_, Imp>(mouse_dragged),
                drag_types,
            );
            ffi::class_replaceMethod(
                class as *const AnyClass as *mut AnyClass,
                up_selector,
                std::mem::transmute::<_, Imp>(mouse_up),
                up_types,
            );
        }
        true
    }

    fn install_drop_methods(class: &'static AnyClass) -> bool {
        let dragging_entered: extern "C-unwind" fn(_, _, _) -> _ = dragging_entered;
        let prepare_for_drag_operation: extern "C-unwind" fn(_, _, _) -> _ =
            prepare_for_drag_operation;
        let perform_drag_operation: extern "C-unwind" fn(_, _, _) -> _ = perform_drag_operation;
        // SAFETY: Each replacement uses the original method's type encoding and an
        // ABI-compatible function. class_replaceMethod adds the override only to
        // winit's concrete window class and leaves NSWindow's runtime identity intact.
        unsafe {
            replace_method(
                class,
                sel!(draggingEntered:),
                std::mem::transmute::<_, Imp>(dragging_entered),
            ) && replace_method(
                class,
                sel!(prepareForDragOperation:),
                std::mem::transmute::<_, Imp>(prepare_for_drag_operation),
            ) && replace_method(
                class,
                sel!(performDragOperation:),
                std::mem::transmute::<_, Imp>(perform_drag_operation),
            )
        }
    }

    unsafe fn replace_method(class: &'static AnyClass, selector: Sel, implementation: Imp) -> bool {
        let method = unsafe { ffi::class_getInstanceMethod(class, selector) };
        if method.is_null() {
            return false;
        }
        let types = unsafe { ffi::method_getTypeEncoding(method) };
        if types.is_null() {
            return false;
        }
        unsafe {
            ffi::class_replaceMethod(
                class as *const AnyClass as *mut AnyClass,
                selector,
                implementation,
                types,
            );
        }
        true
    }

    extern "C-unwind" fn dragging_entered(
        _window: &AnyObject,
        _command: Sel,
        sender: *mut AnyObject,
    ) -> usize {
        let Some(sender) = (unsafe { sender.as_ref() }) else {
            return 0;
        };
        usize::from(!extract_image_paths(sender).is_empty())
    }

    extern "C-unwind" fn prepare_for_drag_operation(
        _window: &AnyObject,
        _command: Sel,
        sender: *mut AnyObject,
    ) -> Bool {
        let Some(sender) = (unsafe { sender.as_ref() }) else {
            return Bool::NO;
        };
        Bool::new(!extract_image_paths(sender).is_empty())
    }

    extern "C-unwind" fn perform_drag_operation(
        _window: &AnyObject,
        _command: Sel,
        sender: *mut AnyObject,
    ) -> Bool {
        let Some(sender) = (unsafe { sender.as_ref() }) else {
            return Bool::NO;
        };
        let paths = extract_image_paths(sender);
        if paths.is_empty() {
            return Bool::NO;
        }
        // AppKit reports the drop in window coordinates with a bottom-left origin.
        let location: NSPoint = unsafe { msg_send![sender, draggingLocation] };
        let content_view: *mut AnyObject = unsafe { msg_send![_window, contentView] };
        let position = unsafe { content_view.as_ref() }.map(|view| {
            let frame: NSRect = unsafe { msg_send![view, frame] };
            ExternalDropPosition {
                x: location.x as f32,
                y: (frame.size.height - location.y) as f32,
                physical: false,
            }
        });
        queue_external_image_drop(ExternalImageDrop::Paths(paths, position));
        Bool::YES
    }

    extern "C-unwind" fn mouse_dragged(view: &AnyObject, command: Sel, event: *mut AnyObject) {
        let native_drag_started = (unsafe { event.as_ref() })
            .and_then(|event| take_macos_file_drag().map(|path| (event, path)))
            .map(|(event, path)| start_native_file_drag(view, event, path))
            .unwrap_or(false);
        if native_drag_started {
            return;
        }

        let Some(original) = ORIGINAL_MOUSE_DRAGGED.get().copied() else {
            return;
        };
        // SAFETY: The IMP was captured from `mouseDragged:` before installing
        // this ABI-compatible override.
        let original: unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) =
            unsafe { std::mem::transmute(original) };
        unsafe { original(view, command, event) };
    }

    extern "C-unwind" fn mouse_up(view: &AnyObject, command: Sel, event: *mut AnyObject) {
        let _ = take_macos_file_drag();
        let Some(original) = ORIGINAL_MOUSE_UP.get().copied() else {
            return;
        };
        // SAFETY: The IMP was captured from `mouseUp:` before installing this
        // ABI-compatible override.
        let original: unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) =
            unsafe { std::mem::transmute(original) };
        unsafe { original(view, command, event) };
    }

    #[allow(deprecated)]
    fn start_native_file_drag(view: &AnyObject, event: &AnyObject, path: PathBuf) -> bool {
        let Ok(path) = std::fs::canonicalize(&path) else {
            return false;
        };
        // SAFETY: This hook is installed exclusively on winit's NSView class and
        // receives AppKit's NSEvent argument for `mouseDragged:`.
        let view = unsafe { &*(view as *const AnyObject).cast::<NSView>() };
        let event = unsafe { &*(event as *const AnyObject).cast::<NSEvent>() };
        let filename = NSString::from_str(&path.to_string_lossy());
        let source_rect = NSRect::new(event.locationInWindow(), NSSize::new(1.0, 1.0));
        view.dragFile_fromRect_slideBack_event(&filename, source_rect, true, event)
    }

    fn extract_image_paths(sender: &AnyObject) -> Vec<PathBuf> {
        // SAFETY: NSDraggingInfo's `draggingPasteboard` returns an AppKit pasteboard.
        let pasteboard: Retained<NSPasteboard> = unsafe { msg_send![sender, draggingPasteboard] };
        let Some(filenames) = pasteboard.propertyListForType(unsafe { NSFilenamesPboardType })
        else {
            return Vec::new();
        };
        // SAFETY: NSFilenamesPboardType's property list is an NSArray<NSString>.
        let filenames: Retained<NSArray<NSString>> = unsafe { Retained::cast_unchecked(filenames) };

        (0..filenames.count())
            .map(|index| PathBuf::from(filenames.objectAtIndex(index).to_string()))
            .collect()
    }
}

#[cfg(windows)]
mod windows_drop_target {
    use super::{queue_external_image_drop, ExternalDropPosition, ExternalImageDrop};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::{
        cell::{RefCell, UnsafeCell},
        ffi::OsString,
        os::windows::ffi::OsStringExt,
        path::PathBuf,
        ptr,
    };
    use windows::{
        core::{implement, PCWSTR},
        Win32::{
            Foundation::HWND,
            Graphics::Gdi::ScreenToClient,
            System::{
                Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
                DataExchange::RegisterClipboardFormatW,
                Memory::{GlobalLock, GlobalSize, GlobalUnlock},
                Ole::{
                    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium,
                    RevokeDragDrop, CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY,
                    DROPEFFECT_NONE,
                },
                SystemServices::MODIFIERKEYS_FLAGS,
            },
            UI::Shell::{DragQueryFileW, HDROP},
        },
    };

    thread_local! {
        static DROP_TARGET: RefCell<Option<IDropTarget>> = const { RefCell::new(None) };
    }

    pub(super) fn install(window: &slint::Window) -> bool {
        let window_handle = window.window_handle();
        let Ok(window_handle) = window_handle.window_handle() else {
            return false;
        };
        let RawWindowHandle::Win32(handle) = window_handle.as_raw() else {
            return false;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        let target: IDropTarget = NativeImageDropTarget::new(hwnd).into();

        let _ = unsafe { RevokeDragDrop(hwnd) };
        if unsafe { RegisterDragDrop(hwnd, &target) }.is_err() {
            return false;
        }
        DROP_TARGET.with(|slot| {
            slot.replace(Some(target));
        });
        true
    }

    #[implement(IDropTarget)]
    struct NativeImageDropTarget {
        accepted: UnsafeCell<bool>,
        hwnd: HWND,
    }

    impl NativeImageDropTarget {
        fn new(hwnd: HWND) -> Self {
            Self {
                accepted: UnsafeCell::new(false),
                hwnd,
            }
        }
    }

    #[allow(non_snake_case)]
    impl IDropTarget_Impl for NativeImageDropTarget_Impl {
        fn DragEnter(
            &self,
            data_object: windows_core::Ref<'_, IDataObject>,
            _key_state: MODIFIERKEYS_FLAGS,
            _point: &windows::Win32::Foundation::POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let accepted = extract_drop(data_object).is_some();
            unsafe {
                *self.accepted.get() = accepted;
                *effect = if accepted {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
            Ok(())
        }

        fn DragOver(
            &self,
            _key_state: MODIFIERKEYS_FLAGS,
            _point: &windows::Win32::Foundation::POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            unsafe {
                *effect = if *self.accepted.get() {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
            Ok(())
        }

        fn DragLeave(&self) -> windows::core::Result<()> {
            unsafe {
                *self.accepted.get() = false;
            }
            Ok(())
        }

        fn Drop(
            &self,
            data_object: windows_core::Ref<'_, IDataObject>,
            _key_state: MODIFIERKEYS_FLAGS,
            point: &windows::Win32::Foundation::POINTL,
            effect: *mut DROPEFFECT,
        ) -> windows::core::Result<()> {
            let payload = extract_drop(data_object);
            unsafe {
                *self.accepted.get() = false;
                *effect = if payload.is_some() {
                    DROPEFFECT_COPY
                } else {
                    DROPEFFECT_NONE
                };
            }
            if let Some(payload) = payload {
                let mut client_point = windows::Win32::Foundation::POINT {
                    x: point.x,
                    y: point.y,
                };
                let converted = unsafe { ScreenToClient(self.hwnd, &mut client_point) }.as_bool();
                let position = converted.then_some(ExternalDropPosition {
                    x: client_point.x as f32,
                    y: client_point.y as f32,
                    physical: true,
                });
                queue_external_image_drop(with_position(payload, position));
            }
            Ok(())
        }
    }

    fn extract_drop(data_object: windows_core::Ref<'_, IDataObject>) -> Option<ExternalImageDrop> {
        let data_object = data_object.as_ref()?;
        if let Some(paths) = extract_file_paths(data_object) {
            if !paths.is_empty() {
                return Some(ExternalImageDrop::Paths(paths, None));
            }
        }
        extract_browser_text(data_object).map(|text| ExternalImageDrop::Text(text, None))
    }

    fn with_position(
        drop: ExternalImageDrop,
        position: Option<ExternalDropPosition>,
    ) -> ExternalImageDrop {
        match drop {
            ExternalImageDrop::Paths(paths, _) => ExternalImageDrop::Paths(paths, position),
            ExternalImageDrop::Text(text, _) => ExternalImageDrop::Text(text, position),
        }
    }

    fn extract_file_paths(data_object: &IDataObject) -> Option<Vec<PathBuf>> {
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = unsafe { data_object.GetData(&format) }.ok()?;
        let hdrop = HDROP(unsafe { medium.u.hGlobal.0 } as *mut _);
        let item_count = unsafe { DragQueryFileW(hdrop, 0xFFFF_FFFF, None) };
        let mut paths = Vec::with_capacity(item_count as usize);
        for index in 0..item_count {
            let character_count = unsafe { DragQueryFileW(hdrop, index, None) } as usize;
            if character_count == 0 {
                continue;
            }
            let mut buffer = vec![0; character_count + 1];
            unsafe {
                DragQueryFileW(hdrop, index, Some(&mut buffer));
            }
            paths.push(PathBuf::from(OsString::from_wide(
                &buffer[..character_count],
            )));
        }
        unsafe {
            ReleaseStgMedium(&mut medium);
        }
        Some(paths)
    }

    fn extract_browser_text(data_object: &IDataObject) -> Option<String> {
        let formats = [
            (CF_UNICODETEXT.0, TextEncoding::Utf16),
            (
                registered_format("UniformResourceLocatorW"),
                TextEncoding::Utf16,
            ),
            (
                registered_format("UniformResourceLocator"),
                TextEncoding::Auto,
            ),
            (registered_format("text/x-moz-url"), TextEncoding::Utf16),
            (registered_format("text/uri-list"), TextEncoding::Auto),
            (registered_format("text/html"), TextEncoding::Auto),
            (registered_format("HTML Format"), TextEncoding::Auto),
        ];
        formats.into_iter().find_map(|(format, encoding)| {
            (format != 0)
                .then(|| extract_text(data_object, format, encoding))
                .flatten()
                .filter(|text| is_supported_text_drop(text))
        })
    }

    fn is_supported_text_drop(text: &str) -> bool {
        let text = text.trim();
        text.contains("http://")
            || text.contains("https://")
            || text.contains("file://")
            || text.contains("src=\"")
            || text.contains("src='")
            || PathBuf::from(text).is_file()
    }

    fn registered_format(name: &str) -> u16 {
        let wide = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) as u16 }
    }

    #[derive(Clone, Copy)]
    enum TextEncoding {
        Utf16,
        Auto,
    }

    fn extract_text(
        data_object: &IDataObject,
        clipboard_format: u16,
        encoding: TextEncoding,
    ) -> Option<String> {
        let format = FORMATETC {
            cfFormat: clipboard_format,
            ptd: ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let mut medium = unsafe { data_object.GetData(&format) }.ok()?;
        let global = unsafe { medium.u.hGlobal };
        let size = unsafe { GlobalSize(global) };
        let pointer = unsafe { GlobalLock(global) };
        if pointer.is_null() || size == 0 {
            unsafe {
                ReleaseStgMedium(&mut medium);
            }
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) };
        let text = match encoding {
            TextEncoding::Utf16 => decode_utf16(bytes),
            TextEncoding::Auto if looks_like_utf16(bytes) => decode_utf16(bytes),
            TextEncoding::Auto => {
                let end = bytes
                    .iter()
                    .position(|byte| *byte == 0)
                    .unwrap_or(bytes.len());
                String::from_utf8_lossy(&bytes[..end]).to_string()
            }
        };
        let _ = unsafe { GlobalUnlock(global) };
        unsafe {
            ReleaseStgMedium(&mut medium);
        }
        let text = text.trim_matches(char::from(0)).trim().to_string();
        (!text.is_empty()).then_some(text)
    }

    fn looks_like_utf16(bytes: &[u8]) -> bool {
        bytes.len() >= 4
            && bytes
                .iter()
                .skip(1)
                .step_by(2)
                .take(24)
                .filter(|byte| **byte == 0)
                .count()
                >= 2
    }

    fn decode_utf16(bytes: &[u8]) -> String {
        let units = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        String::from_utf16_lossy(&units)
    }
}
