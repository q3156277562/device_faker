use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, atomic::AtomicBool},
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

pub static FAKE_PROPS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
pub static IS_FULL_MODE: AtomicBool = AtomicBool::new(false);
pub static ACTIVE_RESET_SESSION: Mutex<Option<ActiveResetSession>> = Mutex::new(None);
pub static ORIGINAL_NATIVE_GET_ONE_ARG: Mutex<Option<OriginalNativeGetOneArg>> =
    Mutex::new(None);
pub static ORIGINAL_NATIVE_GET: Mutex<Option<OriginalNativeGet>> = Mutex::new(None);

#[derive(Clone)]
pub struct ActiveResetSession {
    pub package: String,
    pub backups: HashMap<String, String>,
}
