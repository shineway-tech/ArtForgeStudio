use std::ffi::c_void;
use std::io::{Error, ErrorKind, Result};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use std::sync::atomic::{fence, AtomicU32, Ordering};

type Bool = i32;
type HGlobal = *mut c_void;
type HResult = i32;

const S_OK: HResult = 0;
const S_FALSE: HResult = 1;
const E_NOINTERFACE: HResult = 0x80004002u32 as i32;
const E_NOTIMPL: HResult = 0x80004001u32 as i32;
const E_POINTER: HResult = 0x80004003u32 as i32;
const DV_E_FORMATETC: HResult = 0x80040064u32 as i32;
const OLE_E_ADVISENOTSUPPORTED: HResult = 0x80040003u32 as i32;

const DRAGDROP_S_DROP: HResult = 0x00040100;
const DRAGDROP_S_CANCEL: HResult = 0x00040101;
const DRAGDROP_S_USEDEFAULTCURSORS: HResult = 0x00040102;

const DROPEFFECT_COPY: u32 = 1;
const MK_LBUTTON: u32 = 0x0001;
const VK_LBUTTON: i32 = 0x01;

const CF_HDROP: u16 = 15;
const DATADIR_GET: u32 = 1;
const DVASPECT_CONTENT: u32 = 1;
const GMEM_MOVEABLE: u32 = 0x0002;
const TYMED_HGLOBAL: u32 = 1;

const IID_IUNKNOWN: Guid = Guid {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IENUMFORMATETC: Guid = Guid {
    data1: 0x00000103,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IDATAOBJECT: Guid = Guid {
    data1: 0x0000010e,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

const IID_IDROPSOURCE: Guid = Guid {
    data1: 0x00000121,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DropFiles {
    files_offset: u32,
    point: Point,
    non_client_area: Bool,
    wide: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FormatEtc {
    format: u16,
    target_device: *mut c_void,
    aspect: u32,
    index: i32,
    tymed: u32,
}

#[repr(C)]
struct StgMedium {
    tymed: u32,
    data: HGlobal,
    unknown_for_release: *mut c_void,
}

#[repr(C)]
struct DataObjectVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const Guid,
        object: *mut *mut c_void,
    ) -> HResult,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    get_data: unsafe extern "system" fn(
        this: *mut c_void,
        format: *const FormatEtc,
        medium: *mut StgMedium,
    ) -> HResult,
    get_data_here: unsafe extern "system" fn(
        this: *mut c_void,
        format: *const FormatEtc,
        medium: *mut StgMedium,
    ) -> HResult,
    query_get_data:
        unsafe extern "system" fn(this: *mut c_void, format: *const FormatEtc) -> HResult,
    get_canonical_format_etc: unsafe extern "system" fn(
        this: *mut c_void,
        format_in: *const FormatEtc,
        format_out: *mut FormatEtc,
    ) -> HResult,
    set_data: unsafe extern "system" fn(
        this: *mut c_void,
        format: *const FormatEtc,
        medium: *mut StgMedium,
        release: Bool,
    ) -> HResult,
    enum_format_etc: unsafe extern "system" fn(
        this: *mut c_void,
        direction: u32,
        enum_format: *mut *mut c_void,
    ) -> HResult,
    d_advise: unsafe extern "system" fn(
        this: *mut c_void,
        format: *const FormatEtc,
        advf: u32,
        advise_sink: *mut c_void,
        connection: *mut u32,
    ) -> HResult,
    d_unadvise: unsafe extern "system" fn(this: *mut c_void, connection: u32) -> HResult,
    enum_d_advise:
        unsafe extern "system" fn(this: *mut c_void, enum_advise: *mut *mut c_void) -> HResult,
}

#[repr(C)]
struct DropSourceVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const Guid,
        object: *mut *mut c_void,
    ) -> HResult,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    query_continue_drag: unsafe extern "system" fn(
        this: *mut c_void,
        escape_pressed: Bool,
        key_state: u32,
    ) -> HResult,
    give_feedback: unsafe extern "system" fn(this: *mut c_void, effect: u32) -> HResult,
}

#[repr(C)]
struct EnumFormatEtcVtbl {
    query_interface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const Guid,
        object: *mut *mut c_void,
    ) -> HResult,
    add_ref: unsafe extern "system" fn(this: *mut c_void) -> u32,
    release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    next: unsafe extern "system" fn(
        this: *mut c_void,
        count: u32,
        formats: *mut FormatEtc,
        fetched: *mut u32,
    ) -> HResult,
    skip: unsafe extern "system" fn(this: *mut c_void, count: u32) -> HResult,
    reset: unsafe extern "system" fn(this: *mut c_void) -> HResult,
    clone: unsafe extern "system" fn(this: *mut c_void, cloned: *mut *mut c_void) -> HResult,
}

#[repr(C)]
struct DataObject {
    vtbl: *const DataObjectVtbl,
    ref_count: AtomicU32,
    wide_path: Vec<u16>,
}

#[repr(C)]
struct DropSource {
    vtbl: *const DropSourceVtbl,
    ref_count: AtomicU32,
}

#[repr(C)]
struct EnumFormatEtc {
    vtbl: *const EnumFormatEtcVtbl,
    ref_count: AtomicU32,
    format: FormatEtc,
    index: AtomicU32,
}

#[link(name = "ole32")]
extern "system" {
    fn OleInitialize(reserved: *const c_void) -> HResult;
    fn OleUninitialize();
    fn DoDragDrop(
        data_object: *mut c_void,
        drop_source: *mut c_void,
        ok_effects: u32,
        effect: *mut u32,
    ) -> HResult;
}

#[link(name = "kernel32")]
extern "system" {
    fn GlobalAlloc(flags: u32, bytes: usize) -> HGlobal;
    fn GlobalFree(memory: HGlobal) -> HGlobal;
    fn GlobalLock(memory: HGlobal) -> *mut c_void;
    fn GlobalUnlock(memory: HGlobal) -> Bool;
}

#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(virtual_key: i32) -> i16;
}

static DATA_OBJECT_VTBL: DataObjectVtbl = DataObjectVtbl {
    query_interface: data_query_interface,
    add_ref: data_add_ref,
    release: data_release,
    get_data: data_get_data,
    get_data_here,
    query_get_data: data_query_get_data,
    get_canonical_format_etc,
    set_data,
    enum_format_etc,
    d_advise,
    d_unadvise,
    enum_d_advise,
};

static DROP_SOURCE_VTBL: DropSourceVtbl = DropSourceVtbl {
    query_interface: drop_query_interface,
    add_ref: drop_add_ref,
    release: drop_release,
    query_continue_drag,
    give_feedback,
};

static ENUM_FORMAT_VTBL: EnumFormatEtcVtbl = EnumFormatEtcVtbl {
    query_interface: enum_query_interface,
    add_ref: enum_add_ref,
    release: enum_release,
    next: enum_next,
    skip: enum_skip,
    reset: enum_reset,
    clone: enum_clone,
};

pub fn start(path: &Path) -> Result<()> {
    let path = path.canonicalize()?;
    if !path.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("not a file: {}", path.display()),
        ));
    }

    let _ole = OleGuard::initialize()?;
    let data_object = DataObject::new_raw(&path);
    let drop_source = DropSource::new_raw();
    let mut effect = 0;

    let hr = unsafe {
        DoDragDrop(
            data_object as *mut c_void,
            drop_source as *mut c_void,
            DROPEFFECT_COPY,
            &mut effect,
        )
    };

    unsafe {
        ((*(*data_object).vtbl).release)(data_object as *mut c_void);
        ((*(*drop_source).vtbl).release)(drop_source as *mut c_void);
    }

    if failed(hr) {
        return Err(hresult_error("DoDragDrop", hr));
    }

    let _ = (DRAGDROP_S_DROP, DRAGDROP_S_CANCEL, effect);
    Ok(())
}

struct OleGuard;

impl OleGuard {
    fn initialize() -> Result<Self> {
        let hr = unsafe { OleInitialize(ptr::null()) };
        if failed(hr) {
            Err(hresult_error("OleInitialize", hr))
        } else {
            Ok(Self)
        }
    }
}

impl Drop for OleGuard {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

impl DataObject {
    fn new_raw(path: &Path) -> *mut Self {
        let mut wide_path: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide_path.push(0);

        Box::into_raw(Box::new(Self {
            vtbl: &DATA_OBJECT_VTBL,
            ref_count: AtomicU32::new(1),
            wide_path,
        }))
    }
}

impl DropSource {
    fn new_raw() -> *mut Self {
        Box::into_raw(Box::new(Self {
            vtbl: &DROP_SOURCE_VTBL,
            ref_count: AtomicU32::new(1),
        }))
    }
}

impl EnumFormatEtc {
    fn new_raw(index: u32) -> *mut Self {
        Box::into_raw(Box::new(Self {
            vtbl: &ENUM_FORMAT_VTBL,
            ref_count: AtomicU32::new(1),
            format: hdrop_format(),
            index: AtomicU32::new(index),
        }))
    }
}

unsafe extern "system" fn data_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    object: *mut *mut c_void,
) -> HResult {
    query_interface::<DataObject>(
        this,
        riid,
        object,
        &[IID_IUNKNOWN, IID_IDATAOBJECT],
        data_add_ref,
    )
}

unsafe extern "system" fn data_add_ref(this: *mut c_void) -> u32 {
    add_ref::<DataObject>(this)
}

unsafe extern "system" fn data_release(this: *mut c_void) -> u32 {
    release::<DataObject>(this)
}

unsafe extern "system" fn data_get_data(
    this: *mut c_void,
    format: *const FormatEtc,
    medium: *mut StgMedium,
) -> HResult {
    if medium.is_null() {
        return E_POINTER;
    }

    (*medium).tymed = 0;
    (*medium).data = ptr::null_mut();
    (*medium).unknown_for_release = ptr::null_mut();

    let query = data_query_get_data(this, format);
    if query != S_OK {
        return query;
    }

    let data = &*(this as *mut DataObject);
    match create_hdrop_medium(&data.wide_path) {
        Ok(handle) => {
            (*medium).tymed = TYMED_HGLOBAL;
            (*medium).data = handle;
            S_OK
        }
        Err(_) => DV_E_FORMATETC,
    }
}

unsafe extern "system" fn data_query_get_data(
    _this: *mut c_void,
    format: *const FormatEtc,
) -> HResult {
    if is_hdrop_format(format) {
        S_OK
    } else {
        DV_E_FORMATETC
    }
}

unsafe extern "system" fn enum_format_etc(
    _this: *mut c_void,
    direction: u32,
    enum_format: *mut *mut c_void,
) -> HResult {
    if enum_format.is_null() {
        return E_POINTER;
    }
    *enum_format = ptr::null_mut();

    if direction != DATADIR_GET {
        return E_NOTIMPL;
    }

    *enum_format = EnumFormatEtc::new_raw(0) as *mut c_void;
    S_OK
}

unsafe extern "system" fn get_data_here(
    _this: *mut c_void,
    _format: *const FormatEtc,
    _medium: *mut StgMedium,
) -> HResult {
    E_NOTIMPL
}

unsafe extern "system" fn get_canonical_format_etc(
    _this: *mut c_void,
    _format_in: *const FormatEtc,
    format_out: *mut FormatEtc,
) -> HResult {
    if !format_out.is_null() {
        (*format_out).target_device = ptr::null_mut();
    }
    E_NOTIMPL
}

unsafe extern "system" fn set_data(
    _this: *mut c_void,
    _format: *const FormatEtc,
    _medium: *mut StgMedium,
    _release: Bool,
) -> HResult {
    E_NOTIMPL
}

unsafe extern "system" fn d_advise(
    _this: *mut c_void,
    _format: *const FormatEtc,
    _advf: u32,
    _advise_sink: *mut c_void,
    _connection: *mut u32,
) -> HResult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn d_unadvise(_this: *mut c_void, _connection: u32) -> HResult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn enum_d_advise(
    _this: *mut c_void,
    _enum_advise: *mut *mut c_void,
) -> HResult {
    OLE_E_ADVISENOTSUPPORTED
}

unsafe extern "system" fn drop_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    object: *mut *mut c_void,
) -> HResult {
    query_interface::<DropSource>(
        this,
        riid,
        object,
        &[IID_IUNKNOWN, IID_IDROPSOURCE],
        drop_add_ref,
    )
}

unsafe extern "system" fn drop_add_ref(this: *mut c_void) -> u32 {
    add_ref::<DropSource>(this)
}

unsafe extern "system" fn drop_release(this: *mut c_void) -> u32 {
    release::<DropSource>(this)
}

unsafe extern "system" fn query_continue_drag(
    _this: *mut c_void,
    escape_pressed: Bool,
    key_state: u32,
) -> HResult {
    if escape_pressed != 0 {
        DRAGDROP_S_CANCEL
    } else if key_state & MK_LBUTTON == 0 || !left_button_is_down() {
        DRAGDROP_S_DROP
    } else {
        S_OK
    }
}

unsafe extern "system" fn give_feedback(_this: *mut c_void, _effect: u32) -> HResult {
    DRAGDROP_S_USEDEFAULTCURSORS
}

unsafe extern "system" fn enum_query_interface(
    this: *mut c_void,
    riid: *const Guid,
    object: *mut *mut c_void,
) -> HResult {
    query_interface::<EnumFormatEtc>(
        this,
        riid,
        object,
        &[IID_IUNKNOWN, IID_IENUMFORMATETC],
        enum_add_ref,
    )
}

unsafe extern "system" fn enum_add_ref(this: *mut c_void) -> u32 {
    add_ref::<EnumFormatEtc>(this)
}

unsafe extern "system" fn enum_release(this: *mut c_void) -> u32 {
    release::<EnumFormatEtc>(this)
}

unsafe extern "system" fn enum_next(
    this: *mut c_void,
    count: u32,
    formats: *mut FormatEtc,
    fetched: *mut u32,
) -> HResult {
    if formats.is_null() {
        return E_POINTER;
    }

    let this = &*(this as *mut EnumFormatEtc);
    let current = this.index.load(Ordering::Relaxed);
    if current > 0 || count == 0 {
        if !fetched.is_null() {
            *fetched = 0;
        }
        return S_FALSE;
    }

    *formats = this.format;
    this.index.store(1, Ordering::Relaxed);
    if !fetched.is_null() {
        *fetched = 1;
    }

    if count == 1 {
        S_OK
    } else {
        S_FALSE
    }
}

unsafe extern "system" fn enum_skip(this: *mut c_void, count: u32) -> HResult {
    let this = &*(this as *mut EnumFormatEtc);
    let previous = this.index.swap(1, Ordering::Relaxed);
    if count > 0 && previous == 0 {
        S_OK
    } else {
        S_FALSE
    }
}

unsafe extern "system" fn enum_reset(this: *mut c_void) -> HResult {
    let this = &*(this as *mut EnumFormatEtc);
    this.index.store(0, Ordering::Relaxed);
    S_OK
}

unsafe extern "system" fn enum_clone(this: *mut c_void, cloned: *mut *mut c_void) -> HResult {
    if cloned.is_null() {
        return E_POINTER;
    }

    let this = &*(this as *mut EnumFormatEtc);
    *cloned = EnumFormatEtc::new_raw(this.index.load(Ordering::Relaxed)) as *mut c_void;
    S_OK
}

unsafe fn query_interface<T>(
    this: *mut c_void,
    riid: *const Guid,
    object: *mut *mut c_void,
    supported: &[Guid],
    add_ref_fn: unsafe extern "system" fn(*mut c_void) -> u32,
) -> HResult {
    if object.is_null() {
        return E_POINTER;
    }
    *object = ptr::null_mut();

    if riid.is_null() || !supported.iter().any(|iid| *iid == *riid) {
        return E_NOINTERFACE;
    }

    *object = this;
    add_ref_fn(this);
    let _ = std::marker::PhantomData::<T>;
    S_OK
}

unsafe fn add_ref<T>(this: *mut c_void) -> u32 {
    let object = &*(this as *mut ComObjectHeader<T>);
    object.ref_count.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe fn release<T>(this: *mut c_void) -> u32 {
    let object = &*(this as *mut ComObjectHeader<T>);
    let count = object.ref_count.fetch_sub(1, Ordering::Release) - 1;
    if count == 0 {
        fence(Ordering::Acquire);
        drop(Box::from_raw(this as *mut T));
    }
    count
}

#[repr(C)]
struct ComObjectHeader<T> {
    _vtbl: *const c_void,
    ref_count: AtomicU32,
    _marker: std::marker::PhantomData<T>,
}

fn create_hdrop_medium(wide_path: &[u16]) -> Result<HGlobal> {
    let payload_units = wide_path.len() + 1;
    let total_bytes = size_of::<DropFiles>() + payload_units * size_of::<u16>();
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, total_bytes) };
    if handle.is_null() {
        return Err(Error::last_os_error());
    }

    let memory = unsafe { GlobalLock(handle) };
    if memory.is_null() {
        unsafe {
            GlobalFree(handle);
        }
        return Err(Error::last_os_error());
    }

    unsafe {
        let drop_files = memory as *mut DropFiles;
        (*drop_files).files_offset = size_of::<DropFiles>() as u32;
        (*drop_files).point = Point { x: 0, y: 0 };
        (*drop_files).non_client_area = 0;
        (*drop_files).wide = 1;

        let files = (memory as *mut u8).add(size_of::<DropFiles>()) as *mut u16;
        ptr::copy_nonoverlapping(wide_path.as_ptr(), files, wide_path.len());
        *files.add(wide_path.len()) = 0;
        GlobalUnlock(handle);
    }

    Ok(handle)
}

fn hdrop_format() -> FormatEtc {
    FormatEtc {
        format: CF_HDROP,
        target_device: ptr::null_mut(),
        aspect: DVASPECT_CONTENT,
        index: -1,
        tymed: TYMED_HGLOBAL,
    }
}

fn left_button_is_down() -> bool {
    unsafe { GetAsyncKeyState(VK_LBUTTON) & 0x8000u16 as i16 != 0 }
}

unsafe fn is_hdrop_format(format: *const FormatEtc) -> bool {
    if format.is_null() {
        return false;
    }

    let format = *format;
    format.format == CF_HDROP
        && format.aspect == DVASPECT_CONTENT
        && format.index == -1
        && format.tymed & TYMED_HGLOBAL != 0
}

fn failed(hr: HResult) -> bool {
    hr < 0
}

fn hresult_error(context: &str, hr: HResult) -> Error {
    Error::new(
        ErrorKind::Other,
        format!("{context} failed: HRESULT 0x{:08X}", hr as u32),
    )
}
