#[cfg(target_os = "windows")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "windows")]
pub fn start_thumbnail_drag_preview(path: PathBuf) -> bool {
    if !path.is_file() {
        return false;
    }
    std::thread::spawn(move || {
        let _ = windows_preview::run(path);
    });
    true
}

#[cfg(target_os = "windows")]
pub fn start_thumbnail_file_drag(path: PathBuf) -> bool {
    if !path.is_file() {
        return false;
    }
    // OLE drag-and-drop must begin on the STA/UI thread that received the
    // pointer event. The winit GPU backend captures the mouse while dragging,
    // so release that capture before handing control to DoDragDrop.
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
    }
    windows_file_drag::run(path).is_ok()
}

#[cfg(not(target_os = "windows"))]
pub fn start_thumbnail_drag_preview(_path: PathBuf) -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub fn start_thumbnail_file_drag(_path: PathBuf) -> bool {
    false
}

#[cfg(target_os = "windows")]
mod windows_preview {
    use super::*;
    use image::imageops::FilterType;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{copy_nonoverlapping, null, null_mut};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
        SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS,
        HBITMAP, HDC, HGDIOBJ,
    };
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos,
        PeekMessageW, RegisterClassW, SetWindowPos, ShowWindow, TranslateMessage,
        UpdateLayeredWindow, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
        SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    };

    const CLASS_NAME: &[u16] = &[
        'A' as u16, 'r' as u16, 't' as u16, 'F' as u16, 'o' as u16, 'r' as u16, 'g' as u16,
        'e' as u16, 'D' as u16, 'r' as u16, 'a' as u16, 'g' as u16, 'P' as u16, 'r' as u16,
        'e' as u16, 'v' as u16, 'i' as u16, 'e' as u16, 'w' as u16, 0,
    ];
    const PREVIEW_SIZE: i32 = 220;
    const PREVIEW_ALPHA: u8 = 176;
    const LOOP_MS: u64 = 16;
    const MAX_PREVIEW_SECONDS: u64 = 45;

    struct DibPreview {
        screen_dc: HDC,
        memory_dc: HDC,
        bitmap: HBITMAP,
        old_object: HGDIOBJ,
        size: i32,
    }

    impl Drop for DibPreview {
        fn drop(&mut self) {
            unsafe {
                if !self.memory_dc.is_null() && !self.old_object.is_null() {
                    SelectObject(self.memory_dc, self.old_object);
                }
                if !self.bitmap.is_null() {
                    DeleteObject(self.bitmap as HGDIOBJ);
                }
                if !self.memory_dc.is_null() {
                    DeleteDC(self.memory_dc);
                }
                if !self.screen_dc.is_null() {
                    ReleaseDC(null_mut(), self.screen_dc);
                }
            }
        }
    }

    pub fn run(path: PathBuf) -> Option<()> {
        let pixels = preview_pixels(&path, PREVIEW_SIZE as u32)?;
        unsafe {
            if !left_button_down() {
                return None;
            }
            let preview = create_dib_preview(&pixels, PREVIEW_SIZE)?;
            let hwnd = create_preview_window(PREVIEW_SIZE)?;
            if hwnd.is_null() {
                return None;
            }
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let start = Instant::now();
            loop {
                if !left_button_down() || start.elapsed() > Duration::from_secs(MAX_PREVIEW_SECONDS)
                {
                    break;
                }
                let mut cursor = POINT { x: 0, y: 0 };
                if GetCursorPos(&mut cursor) != 0 {
                    let target = POINT {
                        x: cursor.x - PREVIEW_SIZE / 2,
                        y: cursor.y - PREVIEW_SIZE / 2,
                    };
                    update_window(hwnd, &preview, target);
                    SetWindowPos(
                        hwnd,
                        HWND_TOPMOST,
                        target.x,
                        target.y,
                        PREVIEW_SIZE,
                        PREVIEW_SIZE,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    );
                }
                pump_messages();
                std::thread::sleep(Duration::from_millis(LOOP_MS));
            }
            DestroyWindow(hwnd);
        }
        Some(())
    }

    fn preview_pixels(path: &Path, size: u32) -> Option<Vec<u8>> {
        let image = image::open(path).ok()?.to_rgba8();
        let (width, height) = image.dimensions();
        let side = width.min(height);
        let x = (width - side) / 2;
        let y = (height - side) / 2;
        let cropped = image::imageops::crop_imm(&image, x, y, side, side).to_image();
        let resized = image::imageops::resize(&cropped, size, size, FilterType::Lanczos3);
        let mut bgra = Vec::with_capacity((size * size * 4) as usize);
        for pixel in resized.pixels() {
            let original_alpha = pixel[3] as u16;
            let alpha = (original_alpha * PREVIEW_ALPHA as u16 / 255) as u8;
            let premultiply = |value: u8| -> u8 { ((value as u16 * alpha as u16) / 255) as u8 };
            bgra.push(premultiply(pixel[2]));
            bgra.push(premultiply(pixel[1]));
            bgra.push(premultiply(pixel[0]));
            bgra.push(alpha);
        }
        Some(bgra)
    }

    unsafe fn create_dib_preview(pixels: &[u8], size: i32) -> Option<DibPreview> {
        let screen_dc = GetDC(null_mut());
        if screen_dc.is_null() {
            return None;
        }
        let memory_dc = CreateCompatibleDC(screen_dc);
        if memory_dc.is_null() {
            ReleaseDC(null_mut(), screen_dc);
            return None;
        }
        let mut bitmap_info = BITMAPINFO::default();
        bitmap_info.bmiHeader.biSize =
            size_of::<windows_sys::Win32::Graphics::Gdi::BITMAPINFOHEADER>() as u32;
        bitmap_info.bmiHeader.biWidth = size;
        bitmap_info.bmiHeader.biHeight = -size;
        bitmap_info.bmiHeader.biPlanes = 1;
        bitmap_info.bmiHeader.biBitCount = 32;
        bitmap_info.bmiHeader.biCompression = BI_RGB;
        bitmap_info.bmiHeader.biSizeImage = pixels.len() as u32;
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(
            screen_dc,
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
            DeleteDC(memory_dc);
            ReleaseDC(null_mut(), screen_dc);
            return None;
        }
        copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
        let old_object = SelectObject(memory_dc, bitmap as HGDIOBJ);
        Some(DibPreview {
            screen_dc,
            memory_dc,
            bitmap,
            old_object,
            size,
        })
    }

    unsafe fn create_preview_window(size: i32) -> Option<HWND> {
        let instance = GetModuleHandleW(null());
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(preview_window_proc),
            hInstance: instance as _,
            lpszClassName: CLASS_NAME.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&window_class);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            CLASS_NAME.as_ptr(),
            CLASS_NAME.as_ptr(),
            WS_POPUP,
            0,
            0,
            size,
            size,
            null_mut(),
            null_mut(),
            instance as _,
            null(),
        );
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    }

    unsafe extern "system" fn preview_window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe fn update_window(hwnd: HWND, preview: &DibPreview, position: POINT) {
        let source_position = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: preview.size,
            cy: preview.size,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            hwnd,
            preview.screen_dc,
            &position,
            &size,
            preview.memory_dc,
            &source_position,
            0,
            &blend,
            ULW_ALPHA,
        );
    }

    unsafe fn left_button_down() -> bool {
        (GetAsyncKeyState(VK_LBUTTON as i32) & 0x8000u16 as i16) != 0
    }

    unsafe fn pump_messages() {
        let mut msg: MSG = zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_file_drag {
    use std::cell::Cell;
    use std::fmt::Write as _;
    use std::mem::{size_of, ManuallyDrop};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::{copy_nonoverlapping, null_mut};
    use std::sync::OnceLock;

    use windows::core::{implement, Error, Free, Result, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{
        DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
        E_INVALIDARG, E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, POINT, S_FALSE, S_OK,
    };
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumFORMATETC_Impl,
        IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0,
        TYMED_HGLOBAL,
    };
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
    };
    use windows::Win32::System::Ole::{
        DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, OleUninitialize, CF_HDROP,
        CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
    use windows::Win32::UI::Shell::DROPFILES;

    const FORMAT_COUNT: u32 = 5;

    pub fn run(path: PathBuf) -> Result<()> {
        let path = absolute_display_path(path)?;
        unsafe {
            OleInitialize(None)?;
            let result = run_drag(path);
            OleUninitialize();
            result
        }
    }

    unsafe fn run_drag(path: PathBuf) -> Result<()> {
        let data_object: IDataObject = FileDataObject { path }.into();
        let drop_source: IDropSource = FileDropSource.into();
        let mut effect = DROPEFFECT_NONE;
        let _ = DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY, &mut effect);
        Ok(())
    }

    fn absolute_display_path(path: PathBuf) -> Result<PathBuf> {
        if !path.is_file() {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        if path.is_absolute() {
            Ok(path)
        } else {
            let cwd = std::env::current_dir().map_err(|_| Error::from_hresult(E_INVALIDARG))?;
            Ok(cwd.join(path))
        }
    }

    #[implement(IDataObject)]
    struct FileDataObject {
        path: PathBuf,
    }

    impl IDataObject_Impl for FileDataObject_Impl {
        fn GetData(&self, format: *const FORMATETC) -> Result<STGMEDIUM> {
            unsafe {
                if !format_supported(format) {
                    return Err(Error::from_hresult(DV_E_FORMATETC));
                }
                let format = &*format;
                let hglobal = if format.cfFormat == CF_HDROP.0 {
                    create_hdrop_memory(&self.path)?
                } else if format.cfFormat == uri_list_format() {
                    create_text_memory(&file_uri_for_path(&self.path))?
                } else if text_format_supported(format.cfFormat) {
                    create_text_memory(&self.path.display().to_string())?
                } else if format.cfFormat == preferred_drop_effect_format() {
                    create_drop_effect_memory(DROPEFFECT_COPY.0)?
                } else {
                    return Err(Error::from_hresult(DV_E_FORMATETC));
                };
                Ok(STGMEDIUM {
                    tymed: TYMED_HGLOBAL.0 as u32,
                    u: STGMEDIUM_0 { hGlobal: hglobal },
                    pUnkForRelease: ManuallyDrop::new(None),
                })
            }
        }

        fn GetDataHere(&self, _format: *const FORMATETC, _medium: *mut STGMEDIUM) -> Result<()> {
            Err(Error::from_hresult(E_NOTIMPL))
        }

        fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
            unsafe {
                if format_supported(format) {
                    S_OK
                } else {
                    DV_E_FORMATETC
                }
            }
        }

        fn GetCanonicalFormatEtc(
            &self,
            _format_in: *const FORMATETC,
            format_out: *mut FORMATETC,
        ) -> HRESULT {
            unsafe {
                if !format_out.is_null() {
                    (*format_out).ptd = null_mut();
                }
            }
            S_FALSE
        }

        fn SetData(
            &self,
            _format: *const FORMATETC,
            _medium: *const STGMEDIUM,
            _release: windows::core::BOOL,
        ) -> Result<()> {
            Err(Error::from_hresult(E_NOTIMPL))
        }

        fn EnumFormatEtc(&self, direction: u32) -> Result<IEnumFORMATETC> {
            if direction == DATADIR_GET.0 as u32 {
                Ok(FormatEtcEnumerator {
                    index: Cell::new(0),
                }
                .into())
            } else {
                Err(Error::from_hresult(E_INVALIDARG))
            }
        }

        fn DAdvise(
            &self,
            _format: *const FORMATETC,
            _advf: u32,
            _sink: windows::core::Ref<'_, IAdviseSink>,
        ) -> Result<u32> {
            Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
        }

        fn DUnadvise(&self, _connection: u32) -> Result<()> {
            Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
        }

        fn EnumDAdvise(&self) -> Result<IEnumSTATDATA> {
            Err(Error::from_hresult(OLE_E_ADVISENOTSUPPORTED))
        }
    }

    #[implement(IEnumFORMATETC)]
    struct FormatEtcEnumerator {
        index: Cell<u32>,
    }

    impl IEnumFORMATETC_Impl for FormatEtcEnumerator_Impl {
        fn Next(&self, count: u32, items: *mut FORMATETC, fetched: *mut u32) -> HRESULT {
            unsafe {
                if items.is_null() || (count > 1 && fetched.is_null()) {
                    return E_INVALIDARG;
                }

                let mut written = 0;
                while written < count && self.index.get() < FORMAT_COUNT {
                    if let Some(format) = format_at(self.index.get()) {
                        items.add(written as usize).write(format);
                        written += 1;
                    }
                    self.index.set(self.index.get() + 1);
                }

                if !fetched.is_null() {
                    *fetched = written;
                }

                if written == count {
                    S_OK
                } else {
                    S_FALSE
                }
            }
        }

        fn Skip(&self, count: u32) -> Result<()> {
            self.index.set((self.index.get() + count).min(FORMAT_COUNT));
            Ok(())
        }

        fn Reset(&self) -> Result<()> {
            self.index.set(0);
            Ok(())
        }

        fn Clone(&self) -> Result<IEnumFORMATETC> {
            Ok(FormatEtcEnumerator {
                index: Cell::new(self.index.get()),
            }
            .into())
        }
    }

    #[implement(IDropSource)]
    struct FileDropSource;

    impl IDropSource_Impl for FileDropSource_Impl {
        fn QueryContinueDrag(
            &self,
            escape_pressed: windows::core::BOOL,
            key_state: MODIFIERKEYS_FLAGS,
        ) -> HRESULT {
            if escape_pressed.as_bool() {
                return DRAGDROP_S_CANCEL;
            }
            if key_state.0 & MK_LBUTTON.0 == 0 {
                return DRAGDROP_S_DROP;
            }
            S_OK
        }

        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
            DRAGDROP_S_USEDEFAULTCURSORS
        }
    }

    unsafe fn format_supported(format: *const FORMATETC) -> bool {
        if format.is_null() {
            return false;
        }
        let format = &*format;
        (format.cfFormat == CF_HDROP.0
            || format.cfFormat == CF_UNICODETEXT.0
            || format.cfFormat == uri_list_format()
            || format.cfFormat == file_name_w_format()
            || format.cfFormat == preferred_drop_effect_format())
            && format.dwAspect == DVASPECT_CONTENT.0
            && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0
    }

    fn text_format_supported(format: u16) -> bool {
        format == CF_UNICODETEXT.0 || format == file_name_w_format()
    }

    fn format_at(index: u32) -> Option<FORMATETC> {
        match index {
            0 => Some(drag_format()),
            1 => Some(uri_list_format_etc()),
            2 => Some(file_name_w_format_etc()),
            3 => Some(preferred_drop_effect_format_etc()),
            4 => Some(text_format()),
            _ => None,
        }
    }

    fn drag_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn file_name_w_format_etc() -> FORMATETC {
        FORMATETC {
            cfFormat: file_name_w_format(),
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn uri_list_format_etc() -> FORMATETC {
        FORMATETC {
            cfFormat: uri_list_format(),
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn preferred_drop_effect_format_etc() -> FORMATETC {
        FORMATETC {
            cfFormat: preferred_drop_effect_format(),
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn text_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_UNICODETEXT.0,
            ptd: null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn file_name_w_format() -> u16 {
        static FORMAT: OnceLock<u16> = OnceLock::new();
        *FORMAT.get_or_init(|| register_clipboard_format("FileNameW"))
    }

    fn uri_list_format() -> u16 {
        static FORMAT: OnceLock<u16> = OnceLock::new();
        *FORMAT.get_or_init(|| register_clipboard_format("text/uri-list"))
    }

    fn preferred_drop_effect_format() -> u16 {
        static FORMAT: OnceLock<u16> = OnceLock::new();
        *FORMAT.get_or_init(|| register_clipboard_format("Preferred DropEffect"))
    }

    fn register_clipboard_format(name: &str) -> u16 {
        let mut wide_name: Vec<u16> = name.encode_utf16().collect();
        wide_name.push(0);
        unsafe { RegisterClipboardFormatW(PCWSTR(wide_name.as_ptr())) as u16 }
    }

    fn file_uri_for_path(path: &Path) -> String {
        let path_text = path.display().to_string().replace('\\', "/");
        format!("file:///{}\r\n", percent_encode_uri_path(&path_text))
    }

    fn percent_encode_uri_path(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.as_bytes() {
            let keep = byte.is_ascii_alphanumeric()
                || matches!(*byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':');
            if keep {
                encoded.push(*byte as char);
            } else {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
        encoded
    }

    unsafe fn create_text_memory(text: &str) -> Result<windows::Win32::Foundation::HGLOBAL> {
        let mut wide_text: Vec<u16> = text.encode_utf16().collect();
        wide_text.push(0);
        let data_len = wide_text.len() * size_of::<u16>();
        let hglobal = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, data_len)?;
        let data_ptr = GlobalLock(hglobal).cast::<u16>();
        if data_ptr.is_null() {
            let mut free_handle = hglobal;
            free_handle.free();
            return Err(Error::from_win32());
        }
        copy_nonoverlapping(wide_text.as_ptr(), data_ptr, wide_text.len());
        let _ = GlobalUnlock(hglobal);
        Ok(hglobal)
    }

    unsafe fn create_drop_effect_memory(
        effect: u32,
    ) -> Result<windows::Win32::Foundation::HGLOBAL> {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size_of::<u32>())?;
        let data_ptr = GlobalLock(hglobal).cast::<u32>();
        if data_ptr.is_null() {
            let mut free_handle = hglobal;
            free_handle.free();
            return Err(Error::from_win32());
        }
        data_ptr.write(effect);
        let _ = GlobalUnlock(hglobal);
        Ok(hglobal)
    }

    unsafe fn create_hdrop_memory(path: &Path) -> Result<windows::Win32::Foundation::HGLOBAL> {
        let mut wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide_path.push(0);

        let header_size = size_of::<DROPFILES>();
        let data_len = header_size + (wide_path.len() + 1) * size_of::<u16>();
        let hglobal = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, data_len)?;
        let data_ptr = GlobalLock(hglobal).cast::<u8>();
        if data_ptr.is_null() {
            let mut free_handle = hglobal;
            free_handle.free();
            return Err(Error::from_win32());
        }

        let dropfiles = DROPFILES {
            pFiles: header_size as u32,
            pt: POINT { x: 0, y: 0 },
            fNC: false.into(),
            fWide: true.into(),
        };
        (data_ptr as *mut DROPFILES).write_unaligned(dropfiles);

        let path_ptr = data_ptr.add(header_size) as *mut u16;
        copy_nonoverlapping(wide_path.as_ptr(), path_ptr, wide_path.len());
        path_ptr.add(wide_path.len()).write(0);

        let _ = GlobalUnlock(hglobal);
        Ok(hglobal)
    }
}
