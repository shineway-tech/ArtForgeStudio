use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
pub(crate) enum ExternalImageDrop {
    Paths(Vec<PathBuf>),
    Text(String),
}

static EXTERNAL_IMAGE_DROPS: OnceLock<Mutex<Vec<ExternalImageDrop>>> = OnceLock::new();

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

#[cfg(windows)]
pub(crate) fn install_external_image_drop_target(window: &slint::Window) -> bool {
    windows_drop_target::install(window)
}

#[cfg(not(windows))]
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
    let icon_data = NSData::with_bytes(include_bytes!("../assets/app-icon.png"));
    let icon = NSImage::initWithData(NSImage::alloc(), &icon_data)
        .context("decode embedded macOS application icon")?;
    let application = NSApplication::sharedApplication(main_thread);

    // SAFETY: AppKit retains the supplied NSImage and this runs on the main thread.
    unsafe { application.setApplicationIconImage(Some(&icon)) };
    Ok(())
}

#[cfg(windows)]
mod windows_drop_target {
    use super::{queue_external_image_drop, ExternalImageDrop};
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
        let target: IDropTarget = NativeImageDropTarget::new().into();

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
    }

    impl NativeImageDropTarget {
        fn new() -> Self {
            Self {
                accepted: UnsafeCell::new(false),
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
            _point: &windows::Win32::Foundation::POINTL,
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
                queue_external_image_drop(payload);
            }
            Ok(())
        }
    }

    fn extract_drop(data_object: windows_core::Ref<'_, IDataObject>) -> Option<ExternalImageDrop> {
        let data_object = data_object.as_ref()?;
        if let Some(paths) = extract_file_paths(data_object) {
            if !paths.is_empty() {
                return Some(ExternalImageDrop::Paths(paths));
            }
        }
        extract_browser_text(data_object).map(ExternalImageDrop::Text)
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
