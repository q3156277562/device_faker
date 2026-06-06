use std::{
    collections::HashMap,
    ffi::CString,
    ptr,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, AtomicPtr, Ordering},
    },
};

/// 用于恢复真实属性值的 native_get(String) 原始函数签名。
pub type OriginalNativeGetOneArg = unsafe extern "C" fn(
    env: *mut jni::sys::JNIEnv,
    class: jni::sys::jclass,
    key: jni::sys::jstring,
) -> jni::sys::jstring;

/// 用于恢复真实属性值的 native_get(String, String) 原始函数签名。
pub type OriginalNativeGet = unsafe extern "C" fn(
    env: *mut jni::sys::JNIEnv,
    class: jni::sys::jclass,
    key: jni::sys::jstring,
    def: jni::sys::jstring,
) -> jni::sys::jstring;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimezoneOverrideKind {
    Inactive,
    Deleted,
    Value,
}

#[derive(Default)]
pub struct ProcessTimezoneState {
    override_active: bool,
    timezone_id: Option<String>,
    timezone_cstring: Option<CString>,
    timezone_utf16: Option<Vec<u16>>,
}

impl ProcessTimezoneState {
    fn set_timezone_override(&mut self, timezone: Option<&str>) {
        self.override_active = timezone.is_some();
        let timezone = timezone.filter(|tz| !tz.is_empty() && *tz != "__DELETE__");

        self.timezone_id = timezone.map(str::to_owned);
        self.timezone_cstring = timezone.and_then(|tz| CString::new(tz).ok());
        self.timezone_utf16 = timezone.map(|tz| {
            let mut encoded: Vec<u16> = tz.encode_utf16().collect();
            encoded.push(0);
            encoded
        });
    }

    fn override_kind(&self) -> TimezoneOverrideKind {
        if !self.override_active {
            return TimezoneOverrideKind::Inactive;
        }

        if self.timezone_id.is_some() {
            TimezoneOverrideKind::Value
        } else {
            TimezoneOverrideKind::Deleted
        }
    }
}

pub static FAKE_PROPS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub static PROCESS_TIMEZONE_STATE: LazyLock<Mutex<ProcessTimezoneState>> =
    LazyLock::new(|| Mutex::new(ProcessTimezoneState::default()));
pub static TIMEZONE_PROP_INFO: AtomicPtr<libc::c_void> = AtomicPtr::new(ptr::null_mut());
pub static IS_FULL_MODE: AtomicBool = AtomicBool::new(false);
pub static ACTIVE_RESET_SESSION: Mutex<Option<ActiveResetSession>> = Mutex::new(None);
pub static ORIGINAL_NATIVE_GET_ONE_ARG: Mutex<Option<OriginalNativeGetOneArg>> =
    Mutex::new(None);
pub static ORIGINAL_NATIVE_GET: Mutex<Option<OriginalNativeGet>> = Mutex::new(None);

pub fn set_process_timezone_override(timezone: Option<&str>) {
    PROCESS_TIMEZONE_STATE
        .lock()
        .unwrap()
        .set_timezone_override(timezone);
    TIMEZONE_PROP_INFO.store(ptr::null_mut(), Ordering::Relaxed);
}

pub fn timezone_override_kind() -> TimezoneOverrideKind {
    PROCESS_TIMEZONE_STATE.lock().unwrap().override_kind()
}

pub fn timezone_override_value() -> Option<String> {
    PROCESS_TIMEZONE_STATE.lock().unwrap().timezone_id.clone()
}

pub fn timezone_override_cstr_ptr() -> Option<*const libc::c_char> {
    PROCESS_TIMEZONE_STATE
        .lock()
        .unwrap()
        .timezone_cstring
        .as_ref()
        .map(|value| value.as_ptr())
}

pub fn timezone_override_utf16() -> Option<Vec<u16>> {
    PROCESS_TIMEZONE_STATE.lock().unwrap().timezone_utf16.clone()
}

pub fn store_timezone_prop_info(prop_info: *const libc::c_void) {
    TIMEZONE_PROP_INFO.store(prop_info.cast_mut(), Ordering::Relaxed);
}

pub fn is_timezone_prop_info(prop_info: *const libc::c_void) -> bool {
    !prop_info.is_null() && TIMEZONE_PROP_INFO.load(Ordering::Relaxed).cast_const() == prop_info
}

#[derive(Clone)]
pub struct ActiveResetSession {
    pub package: String,
    pub backups: HashMap<String, String>,
}
