use std::{
    ffi::{CStr, CString},
    ptr,
};

use anyhow::Context;
use jni::{
    Env, EnvUnowned, jni_sig, jni_str,
    objects::{JClass, JString, JValue},
    strings::JNIStr,
    sys::JNINativeMethod,
};
use zygisk_api::api::{V4, ZygiskApi};

use crate::{
    config::MergedAppConfig,
    state::{
        FAKE_PROPS, ORIGINAL_NATIVE_GET, ORIGINAL_NATIVE_GET_ONE_ARG, OriginalNativeGet,
        OriginalNativeGetOneArg, TimezoneOverrideKind, is_timezone_prop_info,
        set_process_timezone_override, store_timezone_prop_info, timezone_override_cstr_ptr,
        timezone_override_kind, timezone_override_utf16, timezone_override_value,
    },
};

static mut ORIGINAL_SYSTEM_PROPERTY_GET: Option<
    unsafe extern "C" fn(*const libc::c_char, *mut libc::c_char) -> libc::c_int,
> = None;
static mut ORIGINAL_PROPERTY_GET: Option<
    unsafe extern "C" fn(
        *const libc::c_char,
        *mut libc::c_char,
        *const libc::c_char,
    ) -> libc::c_int,
> = None;
static mut ORIGINAL_GETENV: Option<
    unsafe extern "C" fn(*const libc::c_char) -> *mut libc::c_char,
> = None;
static mut ORIGINAL_DLSYM: Option<
    unsafe extern "C" fn(*mut libc::c_void, *const libc::c_char) -> *mut libc::c_void,
> = None;

type PropInfo = libc::c_void;
type SystemPropertyReadCallback =
    unsafe extern "C" fn(*mut libc::c_void, *const libc::c_char, *const libc::c_char, u32);

static mut ORIGINAL_SYSTEM_PROPERTY_FIND: Option<
    unsafe extern "C" fn(*const libc::c_char) -> *const PropInfo,
> = None;
static mut ORIGINAL_SYSTEM_PROPERTY_READ_CALLBACK: Option<
    unsafe extern "C" fn(
        *const PropInfo,
        Option<SystemPropertyReadCallback>,
        *mut libc::c_void,
    ),
> = None;
static mut ORIGINAL_SYSTEM_PROPERTY_READ: Option<
    unsafe extern "C" fn(*const PropInfo, *mut libc::c_char, *mut libc::c_char) -> libc::c_int,
> = None;

static mut ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE: Option<
    unsafe extern "C" fn(*mut u16, i32, *mut i32) -> i32,
> = None;
static mut ORIGINAL_UCAL_SET_DEFAULT_TIME_ZONE: Option<unsafe extern "C" fn(*const u16, *mut i32)> =
    None;

unsafe extern "C" {
    fn setenv(
        name: *const libc::c_char,
        value: *const libc::c_char,
        overwrite: libc::c_int,
    ) -> libc::c_int;
    fn unsetenv(name: *const libc::c_char) -> libc::c_int;
}

const PROPERTY_VALUE_MAX_LEN: usize = 91;
const TIMEZONE_PROPERTY_NAME: &str = "persist.sys.timezone";
const U_ZERO_ERROR: i32 = 0;
const U_BUFFER_OVERFLOW_ERROR: i32 = 15;

#[cfg(not(target_os = "windows"))]
unsafe extern "C" {
    fn tzset();
}

#[cfg(target_os = "windows")]
unsafe extern "C" {
    fn _tzset();
}

/// 根据合并配置 Hook android.os.Build 的静态字段。
pub fn hook_build_fields(
    env: &mut EnvUnowned,
    merged_config: &MergedAppConfig,
) -> anyhow::Result<()> {
    env.with_env(|jenv| -> Result<(), jni::errors::Error> {
        let build_class = jenv.find_class(jni_str!("android/os/Build"))?;

        if let Some(manufacturer) = &merged_config.manufacturer
            && !manufacturer.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("MANUFACTURER"), manufacturer)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(brand) = &merged_config.brand
            && !brand.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("BRAND"), brand)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(model) = &merged_config.model
            && !model.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("MODEL"), model)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(device) = &merged_config.device
            && !device.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("DEVICE"), device)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(product) = &merged_config.product
            && !product.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("PRODUCT"), product)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(fingerprint) = &merged_config.fingerprint
            && !fingerprint.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("FINGERPRINT"), fingerprint)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        if let Some(build_id) = &merged_config.build_id
            && !build_id.is_empty()
        {
            set_build_field(jenv, &build_class, jni_str!("ID"), build_id)
                .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;
        }

        hook_version_fields(jenv, &build_class, merged_config)
            .map_err(|_e| jni::errors::Error::JniCall(jni::errors::JniError::Unknown))?;

        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
    Ok(())
}

fn hook_version_fields(
    env: &mut Env,
    _build_class: &JClass,
    merged_config: &MergedAppConfig,
) -> anyhow::Result<()> {
    let version_class = env
        .find_class(jni_str!("android/os/Build$VERSION"))
        .context("Failed to find Build.VERSION class")?;

    if let Some(android_version) = &merged_config.android_version
        && !android_version.is_empty()
    {
        set_build_field(env, &version_class, jni_str!("RELEASE"), android_version)?;
    }

    if let Some(sdk_int) = merged_config.sdk_int {
        set_build_int_field(env, &version_class, jni_str!("SDK_INT"), sdk_int as i32)?;
    }

    Ok(())
}

/// 设置目标应用进程内的默认时区。
pub fn hook_timezone(
    env: &mut EnvUnowned,
    merged_config: &MergedAppConfig,
) -> anyhow::Result<()> {
    let Some(timezone) = merged_config.timezone.as_deref() else {
        return Ok(());
    };

    if timezone.is_empty() {
        return Ok(());
    }

    apply_process_timezone(timezone);

    if timezone == "__DELETE__" {
        return Ok(());
    }

    env.with_env(|jenv| -> Result<(), jni::errors::Error> {
        let timezone_class = jenv.find_class(jni_str!("java/util/TimeZone"))?;
        let timezone_id = jenv.new_string(timezone)?;
        let timezone_object = jenv
            .call_static_method(
                &timezone_class,
                jni_str!("getTimeZone"),
                jni_sig!("(Ljava/lang/String;)Ljava/util/TimeZone;"),
                &[JValue::Object(&timezone_id)],
            )?
            .l()?;

        jenv.call_static_method(
            &timezone_class,
            jni_str!("setDefault"),
            jni_sig!("(Ljava/util/TimeZone;)V"),
            &[JValue::Object(&timezone_object)],
        )?;

        try_set_android_icu_timezone_default(jenv, timezone);

        let system_class = jenv.find_class(jni_str!("java/lang/System"))?;
        let property_key = jenv.new_string("user.timezone")?;
        let property_value = jenv.new_string(timezone)?;
        jenv.call_static_method(
            &system_class,
            jni_str!("setProperty"),
            jni_sig!("(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"),
            &[JValue::Object(&property_key), JValue::Object(&property_value)],
        )?;

        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
    Ok(())
}

fn apply_process_timezone(timezone: &str) {
    set_process_timezone_override(Some(timezone));

    // SAFETY: This runs during app specialization, before the target app starts executing
    // its own code. Mutating the libc process environment here avoids concurrent env access.
    unsafe {
        if timezone == "__DELETE__" {
            let _ = unsetenv(c"TZ".as_ptr());
        } else if let Some(timezone_ptr) = timezone_override_cstr_ptr() {
            let _ = setenv(c"TZ".as_ptr(), timezone_ptr, 1);
        }
    }

    refresh_process_timezone();
}

fn refresh_process_timezone() {
    #[cfg(not(target_os = "windows"))]
    unsafe {
        tzset();
    }

    #[cfg(target_os = "windows")]
    unsafe {
        _tzset();
    }
}

fn clear_pending_jni_exception(env: &mut Env) {
    let raw_env = env.get_raw();
    if raw_env.is_null() {
        return;
    }

    unsafe {
        if (**raw_env).ExceptionCheck(raw_env) != 0 {
            (**raw_env).ExceptionClear(raw_env);
        }
    }
}

fn try_set_android_icu_timezone_default(env: &mut Env, timezone: &str) {
    let timezone_class = match env.find_class(jni_str!("android/icu/util/TimeZone")) {
        Ok(class) => class,
        Err(_) => {
            clear_pending_jni_exception(env);
            return;
        }
    };

    let timezone_id = match env.new_string(timezone) {
        Ok(value) => value,
        Err(_) => return,
    };

    let timezone_object = match env
        .call_static_method(
            &timezone_class,
            jni_str!("getTimeZone"),
            jni_sig!("(Ljava/lang/String;)Landroid/icu/util/TimeZone;"),
            &[JValue::Object(&timezone_id)],
        )
        .and_then(|value| value.l())
    {
        Ok(value) => value,
        Err(_) => {
            clear_pending_jni_exception(env);
            return;
        }
    };

    if env
        .call_static_method(
            &timezone_class,
            jni_str!("setDefault"),
            jni_sig!("(Landroid/icu/util/TimeZone;)V"),
            &[JValue::Object(&timezone_object)],
        )
        .is_err()
    {
        clear_pending_jni_exception(env);
    }
}

fn set_build_field(
    env: &mut Env,
    build_class: &JClass,
    field_name: &JNIStr,
    value: &str,
) -> anyhow::Result<()> {
    let _field_id = env
        .get_static_field_id(build_class, field_name, jni_sig!("Ljava/lang/String;"))
        .with_context(|| "Failed to get field ID".to_string())?;

    let new_value = env
        .new_string(value)
        .with_context(|| format!("Failed to create string for {value}"))?;

    env.set_static_field(
        build_class,
        field_name,
        jni_sig!("Ljava/lang/String;"),
        JValue::Object(&new_value),
    )
    .with_context(|| "Failed to set field".to_string())?;

    Ok(())
}

fn set_build_int_field(
    env: &mut Env,
    build_class: &JClass,
    field_name: &JNIStr,
    value: i32,
) -> anyhow::Result<()> {
    let _field_id = env
        .get_static_field_id(build_class, field_name, jni_sig!("I"))
        .with_context(|| "Failed to get field ID".to_string())?;

    env.set_static_field(build_class, field_name, jni_sig!("I"), JValue::Int(value))
        .with_context(|| "Failed to set field".to_string())?;

    Ok(())
}

/// Hook SystemProperties.native_get 以截获属性查询。
pub fn hook_system_properties(
    api: &mut ZygiskApi<V4>,
    env: &mut EnvUnowned,
) -> anyhow::Result<()> {
    let mut methods = [
        JNINativeMethod {
            name: c"native_get".as_ptr().cast_mut(),
            signature: c"(Ljava/lang/String;)Ljava/lang/String;"
                .as_ptr()
                .cast_mut(),
            fnPtr: native_get_one_arg_hook as *mut std::ffi::c_void,
        },
        JNINativeMethod {
            name: c"native_get".as_ptr().cast_mut(),
            signature: c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;"
                .as_ptr()
                .cast_mut(),
            fnPtr: native_get_hook as *mut std::ffi::c_void,
        },
    ];

    let class_name = unsafe { JNIStr::from_ptr(c"android/os/SystemProperties".as_ptr()) };

    env.with_env(|jenv| -> Result<(), jni::errors::Error> {
        let env_unowned = unsafe { EnvUnowned::from_raw(jenv.get_raw()) };
        unsafe {
            api.hook_jni_native_methods(env_unowned, class_name, &mut methods);
        }
        Ok(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();

    let original_one_arg_fn_ptr = if methods[0].fnPtr.is_null() {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, OriginalNativeGetOneArg>(methods[0].fnPtr)
        })
    };
    let original_two_arg_fn_ptr = if methods[1].fnPtr.is_null() {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<*mut std::ffi::c_void, OriginalNativeGet>(methods[1].fnPtr)
        })
    };

    *ORIGINAL_NATIVE_GET_ONE_ARG.lock().unwrap() = original_one_arg_fn_ptr;
    *ORIGINAL_NATIVE_GET.lock().unwrap() = original_two_arg_fn_ptr;

    Ok(())
}

/// 为 Hook 提供的 SystemProperties.native_get(String) 替身实现。
pub unsafe extern "C" fn native_get_one_arg_hook(
    env: *mut jni::sys::JNIEnv,
    class: jni::sys::jclass,
    key: jni::sys::jstring,
) -> jni::sys::jstring {
    let mut env_wrapper = unsafe { EnvUnowned::from_raw(env) };

    let result = env_wrapper.with_env(|jenv| -> Result<jni::sys::jstring, jni::errors::Error> {
        let key_jstring = unsafe { JString::from_raw(jenv, key) };
        let key_string = match get_property_key(jenv, &key_jstring) {
            Ok(key_string) => key_string,
            Err(_) => {
                if let Some(orig_fn) = *ORIGINAL_NATIVE_GET_ONE_ARG.lock().unwrap() {
                    return Ok(unsafe { orig_fn(env, class, key) });
                }

                let empty = jenv.new_string("")?;
                return Ok(empty.into_raw());
            }
        };

        if let Some(fake_value) = get_fake_property(&key_string)
            && let Ok(new_string) = jenv.new_string(&fake_value)
        {
            return Ok(new_string.into_raw());
        }

        if is_deleted_fake_property(&key_string) {
            let empty = jenv.new_string("")?;
            return Ok(empty.into_raw());
        }

        if let Some(orig_fn) = *ORIGINAL_NATIVE_GET_ONE_ARG.lock().unwrap() {
            return Ok(unsafe { orig_fn(env, class, key) });
        }

        let empty = jenv.new_string("")?;
        Ok(empty.into_raw())
    });

    result.resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

/// 为 Hook 提供的 SystemProperties.native_get(String, String) 替身实现。
pub unsafe extern "C" fn native_get_hook(
    env: *mut jni::sys::JNIEnv,
    class: jni::sys::jclass,
    key: jni::sys::jstring,
    def: jni::sys::jstring,
) -> jni::sys::jstring {
    let mut env_wrapper = unsafe { EnvUnowned::from_raw(env) };

    let result = env_wrapper.with_env(|jenv| -> Result<jni::sys::jstring, jni::errors::Error> {
        let key_jstring = unsafe { JString::from_raw(jenv, key) };
        let key_string = match get_property_key(jenv, &key_jstring) {
            Ok(key_string) => key_string,
            Err(_) => return Ok(def),
        };

        if let Some(fake_value) = get_fake_property(&key_string)
            && let Ok(new_string) = jenv.new_string(&fake_value)
        {
            return Ok(new_string.into_raw());
        }

        if is_deleted_fake_property(&key_string) {
            return Ok(def);
        }

        if let Some(orig_fn) = *ORIGINAL_NATIVE_GET.lock().unwrap() {
            return Ok(unsafe { orig_fn(env, class, key, def) });
        }

        Ok(def)
    });

    result.resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}

fn get_property_key(env: &mut Env, key: &JString) -> Result<String, jni::errors::Error> {
    Ok(key.mutf8_chars(env)?.to_string())
}

fn get_fake_property(name: &str) -> Option<String> {
    if name == TIMEZONE_PROPERTY_NAME
        && let Some(timezone) = timezone_override_value()
    {
        return Some(timezone);
    }

    FAKE_PROPS.lock().unwrap().get(name).cloned()
}

fn is_deleted_fake_property(name: &str) -> bool {
    name == TIMEZONE_PROPERTY_NAME && timezone_override_kind() == TimezoneOverrideKind::Deleted
}

unsafe fn copy_property_value(value: *mut libc::c_char, prop_value: &str) -> libc::c_int {
    let len = prop_value.len().min(PROPERTY_VALUE_MAX_LEN);
    unsafe {
        ptr::copy_nonoverlapping(prop_value.as_ptr().cast::<libc::c_char>(), value, len);
        value.add(len).write(0);
    }
    len as libc::c_int
}

unsafe fn write_empty_property_value(value: *mut libc::c_char) -> libc::c_int {
    unsafe {
        value.write(0);
    }
    0
}

unsafe fn copy_default_or_empty_property_value(
    value: *mut libc::c_char,
    default_value: *const libc::c_char,
) -> libc::c_int {
    if default_value.is_null() {
        return unsafe { write_empty_property_value(value) };
    }

    let default_str = match unsafe { CStr::from_ptr(default_value).to_str() } {
        Ok(s) => s,
        Err(_) => return unsafe { write_empty_property_value(value) },
    };

    unsafe { copy_property_value(value, default_str) }
}

unsafe fn write_property_name(name: *mut libc::c_char, prop_name: &str) {
    if name.is_null() {
        return;
    }

    unsafe {
        ptr::copy_nonoverlapping(prop_name.as_ptr().cast::<libc::c_char>(), name, prop_name.len());
        name.add(prop_name.len()).write(0);
    }
}

fn adjusted_property_serial(serial: u32, value_len: usize) -> u32 {
    (serial & 0x00ff_ffff) | ((value_len.min(u8::MAX as usize) as u32) << 24)
}

struct PropertyReadHookContext {
    callback: Option<SystemPropertyReadCallback>,
    cookie: *mut libc::c_void,
}

unsafe extern "C" fn timezone_property_read_callback_forwarder(
    context: *mut libc::c_void,
    name: *const libc::c_char,
    _value: *const libc::c_char,
    serial: u32,
) {
    if context.is_null() {
        return;
    }

    let Some(callback) = (unsafe { &mut *context.cast::<PropertyReadHookContext>() }).callback else {
        return;
    };

    let Some(fake_value_ptr) = timezone_override_cstr_ptr() else {
        return;
    };
    let value_len = timezone_override_value().map_or(0, |value| value.len());
    let callback_name = if name.is_null() {
        c"persist.sys.timezone".as_ptr()
    } else {
        name
    };

    unsafe {
        callback(
            (*context.cast::<PropertyReadHookContext>()).cookie,
            callback_name,
            fake_value_ptr,
            adjusted_property_serial(serial, value_len),
        );
    }
}

unsafe extern "C" fn my_system_property_get(
    name: *const libc::c_char,
    value: *mut libc::c_char,
) -> libc::c_int {
    if name.is_null() || value.is_null() {
        return 0;
    }

    let name_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };

    if let Some(fake_value) = get_fake_property(name_str) {
        return unsafe { copy_property_value(value, &fake_value) };
    }

    if is_deleted_fake_property(name_str) {
        return unsafe { write_empty_property_value(value) };
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_GET {
            return orig_fn(name, value);
        }
    }

    0
}

unsafe extern "C" fn my_property_get(
    name: *const libc::c_char,
    value: *mut libc::c_char,
    default_value: *const libc::c_char,
) -> libc::c_int {
    if name.is_null() || value.is_null() {
        return 0;
    }

    let name_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(s) => s,
        Err(_) => return 0,
    };

    if let Some(fake_value) = get_fake_property(name_str) {
        return unsafe { copy_property_value(value, &fake_value) };
    }

    if is_deleted_fake_property(name_str) {
        return unsafe { copy_default_or_empty_property_value(value, default_value) };
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_PROPERTY_GET {
            return orig_fn(name, value, default_value);
        }
    }

    unsafe { copy_default_or_empty_property_value(value, default_value) }
}

unsafe extern "C" fn my_getenv(name: *const libc::c_char) -> *mut libc::c_char {
    if !name.is_null() {
        let env_name = unsafe { CStr::from_ptr(name).to_bytes() };
        if env_name == b"TZ" {
            match timezone_override_kind() {
                TimezoneOverrideKind::Value => {
                    if let Some(timezone_ptr) = timezone_override_cstr_ptr() {
                        return timezone_ptr.cast_mut();
                    }
                }
                TimezoneOverrideKind::Deleted => return ptr::null_mut(),
                TimezoneOverrideKind::Inactive => {}
            }
        }
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_GETENV {
            return orig_fn(name);
        }
    }

    ptr::null_mut()
}

unsafe extern "C" fn my_dlsym(
    handle: *mut libc::c_void,
    symbol: *const libc::c_char,
) -> *mut libc::c_void {
    if !symbol.is_null() {
        let symbol_name = unsafe { CStr::from_ptr(symbol).to_bytes() };
        if let Some(hook) = dlsym_timezone_hook(symbol_name) {
            unsafe {
                if let Some(orig_fn) = ORIGINAL_DLSYM {
                    let original = orig_fn(handle, symbol);
                    store_dlsym_timezone_original(symbol_name, original);
                }
            }
            return hook;
        }
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_DLSYM {
            return orig_fn(handle, symbol);
        }
    }

    ptr::null_mut()
}

fn dlsym_timezone_hook(symbol_name: &[u8]) -> Option<*mut libc::c_void> {
    if timezone_override_kind() == TimezoneOverrideKind::Inactive {
        return None;
    }

    match symbol_name {
        b"getenv" => Some(my_getenv as *mut libc::c_void),
        b"__system_property_get" => Some(my_system_property_get as *mut libc::c_void),
        b"property_get" => Some(my_property_get as *mut libc::c_void),
        b"__system_property_find" => Some(my_system_property_find as *mut libc::c_void),
        b"__system_property_read_callback" => {
            Some(my_system_property_read_callback as *mut libc::c_void)
        }
        b"__system_property_read" => Some(my_system_property_read as *mut libc::c_void),
        _ if symbol_name.starts_with(b"ucal_getDefaultTimeZone")
            && timezone_override_value().is_some() =>
        {
            Some(my_ucal_get_default_time_zone as *mut libc::c_void)
        }
        _ if symbol_name.starts_with(b"ucal_setDefaultTimeZone")
            && timezone_override_value().is_some() =>
        {
            Some(my_ucal_set_default_time_zone as *mut libc::c_void)
        }
        _ => None,
    }
}

unsafe fn store_dlsym_timezone_original(symbol_name: &[u8], original: *mut libc::c_void) {
    if original.is_null() {
        return;
    }

    unsafe {
        match symbol_name {
            b"getenv" => ORIGINAL_GETENV = Some(std::mem::transmute(original)),
            b"__system_property_get" => {
                ORIGINAL_SYSTEM_PROPERTY_GET = Some(std::mem::transmute(original))
            }
            b"property_get" => ORIGINAL_PROPERTY_GET = Some(std::mem::transmute(original)),
            b"__system_property_find" => {
                ORIGINAL_SYSTEM_PROPERTY_FIND = Some(std::mem::transmute(original))
            }
            b"__system_property_read_callback" => {
                ORIGINAL_SYSTEM_PROPERTY_READ_CALLBACK = Some(std::mem::transmute(original))
            }
            b"__system_property_read" => {
                ORIGINAL_SYSTEM_PROPERTY_READ = Some(std::mem::transmute(original))
            }
            _ if symbol_name.starts_with(b"ucal_getDefaultTimeZone") => {
                ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE = Some(std::mem::transmute(original))
            }
            _ if symbol_name.starts_with(b"ucal_setDefaultTimeZone") => {
                ORIGINAL_UCAL_SET_DEFAULT_TIME_ZONE = Some(std::mem::transmute(original))
            }
            _ => {}
        }
    }
}

unsafe extern "C" fn my_system_property_find(name: *const libc::c_char) -> *const PropInfo {
    if name.is_null() {
        return ptr::null();
    }

    let name_str = match unsafe { CStr::from_ptr(name).to_str() } {
        Ok(value) => value,
        Err(_) => return ptr::null(),
    };

    if name_str == TIMEZONE_PROPERTY_NAME {
        match timezone_override_kind() {
            TimezoneOverrideKind::Deleted => return ptr::null(),
            TimezoneOverrideKind::Value => unsafe {
                if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_FIND {
                    let prop_info = orig_fn(name);
                    if !prop_info.is_null() {
                        store_timezone_prop_info(prop_info.cast());
                    }
                    return prop_info;
                }
                return ptr::null();
            },
            TimezoneOverrideKind::Inactive => {}
        }
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_FIND {
            return orig_fn(name);
        }
    }

    ptr::null()
}

unsafe extern "C" fn my_system_property_read_callback(
    prop_info: *const PropInfo,
    callback: Option<SystemPropertyReadCallback>,
    cookie: *mut libc::c_void,
) {
    if !prop_info.is_null() && is_timezone_prop_info(prop_info.cast()) {
        unsafe {
            if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_READ_CALLBACK {
                let mut context = PropertyReadHookContext { callback, cookie };
                orig_fn(
                    prop_info,
                    Some(timezone_property_read_callback_forwarder),
                    (&mut context as *mut PropertyReadHookContext).cast(),
                );
                return;
            }
        }

        if let Some(callback) = callback
            && let Some(fake_value_ptr) = timezone_override_cstr_ptr()
        {
            let value_len = timezone_override_value().map_or(0, |value| value.len());
            unsafe {
                callback(
                    cookie,
                    c"persist.sys.timezone".as_ptr(),
                    fake_value_ptr,
                    adjusted_property_serial(0, value_len),
                );
            }
        }
        return;
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_READ_CALLBACK {
            orig_fn(prop_info, callback, cookie);
        }
    }
}

unsafe extern "C" fn my_system_property_read(
    prop_info: *const PropInfo,
    name: *mut libc::c_char,
    value: *mut libc::c_char,
) -> libc::c_int {
    if prop_info.is_null() || value.is_null() {
        return 0;
    }

    if is_timezone_prop_info(prop_info.cast()) {
        unsafe {
            write_property_name(name, TIMEZONE_PROPERTY_NAME);
        }

        if let Some(fake_value) = timezone_override_value() {
            return unsafe { copy_property_value(value, &fake_value) };
        }

        return unsafe { write_empty_property_value(value) };
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_SYSTEM_PROPERTY_READ {
            return orig_fn(prop_info, name, value);
        }
    }

    0
}

unsafe fn copy_icu_timezone_value(
    result: *mut u16,
    result_capacity: i32,
    timezone_utf16: &[u16],
    ec: *mut i32,
) -> i32 {
    let value = if timezone_utf16.last() == Some(&0) {
        &timezone_utf16[..timezone_utf16.len().saturating_sub(1)]
    } else {
        timezone_utf16
    };
    let value_len = value.len() as i32;

    if !ec.is_null() {
        unsafe {
            *ec = U_ZERO_ERROR;
        }
    }

    if result_capacity <= 0 || result.is_null() {
        if !ec.is_null() {
            unsafe {
                *ec = U_BUFFER_OVERFLOW_ERROR;
            }
        }
        return value_len;
    }

    let capacity = result_capacity as usize;
    let copy_len = value.len().min(capacity.saturating_sub(1));

    unsafe {
        ptr::copy_nonoverlapping(value.as_ptr(), result, copy_len);
        result.add(copy_len).write(0);
    }

    if copy_len < value.len() && !ec.is_null() {
        unsafe {
            *ec = U_BUFFER_OVERFLOW_ERROR;
        }
    }

    value_len
}

unsafe extern "C" fn my_ucal_get_default_time_zone(
    result: *mut u16,
    result_capacity: i32,
    ec: *mut i32,
) -> i32 {
    if let Some(timezone_utf16) = timezone_override_utf16() {
        if ec.is_null() {
            unsafe {
                if let Some(orig_fn) = ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE {
                    return orig_fn(result, result_capacity, ec);
                }
            }
            return 0;
        }

        unsafe {
            if *ec > U_ZERO_ERROR {
                if let Some(orig_fn) = ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE {
                    return orig_fn(result, result_capacity, ec);
                }
                return 0;
            }
        }

        return unsafe { copy_icu_timezone_value(result, result_capacity, &timezone_utf16, ec) };
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE {
            return orig_fn(result, result_capacity, ec);
        }
    }

    0
}

unsafe extern "C" fn my_ucal_set_default_time_zone(_zone_id: *const u16, ec: *mut i32) {
    if timezone_override_utf16().is_some() {
        if !ec.is_null() {
            unsafe {
                if *ec <= U_ZERO_ERROR {
                    *ec = U_ZERO_ERROR;
                }
            }
        }
        return;
    }

    unsafe {
        if let Some(orig_fn) = ORIGINAL_UCAL_SET_DEFAULT_TIME_ZONE {
            orig_fn(_zone_id, ec);
        }
    }
}

pub fn hook_native_property_get(api: &mut ZygiskApi<V4>) -> anyhow::Result<()> {
    let system_symbol = CString::new("__system_property_get").unwrap();
    let property_get_symbol = CString::new("property_get").unwrap();
    let getenv_symbol = CString::new("getenv").unwrap();
    let property_find_symbol = CString::new("__system_property_find").unwrap();
    let property_read_callback_symbol = CString::new("__system_property_read_callback").unwrap();
    let property_read_symbol = CString::new("__system_property_read").unwrap();
    let dlsym_symbol = CString::new("dlsym").unwrap();
    let ucal_get_default_timezone_symbol = CString::new("ucal_getDefaultTimeZone").unwrap();
    let ucal_get_default_timezone_72_symbol = CString::new("ucal_getDefaultTimeZone_72").unwrap();
    let ucal_get_default_timezone_73_symbol = CString::new("ucal_getDefaultTimeZone_73").unwrap();
    let ucal_get_default_timezone_74_symbol = CString::new("ucal_getDefaultTimeZone_74").unwrap();
    let ucal_get_default_timezone_75_symbol = CString::new("ucal_getDefaultTimeZone_75").unwrap();
    let ucal_set_default_timezone_symbol = CString::new("ucal_setDefaultTimeZone").unwrap();
    let ucal_set_default_timezone_72_symbol = CString::new("ucal_setDefaultTimeZone_72").unwrap();
    let ucal_set_default_timezone_73_symbol = CString::new("ucal_setDefaultTimeZone_73").unwrap();
    let ucal_set_default_timezone_74_symbol = CString::new("ucal_setDefaultTimeZone_74").unwrap();
    let ucal_set_default_timezone_75_symbol = CString::new("ucal_setDefaultTimeZone_75").unwrap();
    let timezone_hook_mode = timezone_override_kind();

    #[allow(clippy::missing_transmute_annotations)]
    unsafe {
        let mut system_original: *const () = std::ptr::null();
        api.plt_hook_register(
            0,
            0,
            system_symbol,
            my_system_property_get as *const (),
            &mut system_original,
        );

        let mut property_original: *const () = std::ptr::null();
        api.plt_hook_register(
            0,
            0,
            property_get_symbol,
            my_property_get as *const (),
            &mut property_original,
        );

        let mut getenv_original: *const () = std::ptr::null();
        let mut property_find_original: *const () = std::ptr::null();
        let mut property_read_callback_original: *const () = std::ptr::null();
        let mut property_read_original: *const () = std::ptr::null();
        let mut dlsym_original: *const () = std::ptr::null();
        let mut ucal_get_default_timezone_original: *const () = std::ptr::null();
        let mut ucal_get_default_timezone_72_original: *const () = std::ptr::null();
        let mut ucal_get_default_timezone_73_original: *const () = std::ptr::null();
        let mut ucal_get_default_timezone_74_original: *const () = std::ptr::null();
        let mut ucal_get_default_timezone_75_original: *const () = std::ptr::null();
        let mut ucal_set_default_timezone_original: *const () = std::ptr::null();
        let mut ucal_set_default_timezone_72_original: *const () = std::ptr::null();
        let mut ucal_set_default_timezone_73_original: *const () = std::ptr::null();
        let mut ucal_set_default_timezone_74_original: *const () = std::ptr::null();
        let mut ucal_set_default_timezone_75_original: *const () = std::ptr::null();

        if timezone_hook_mode != TimezoneOverrideKind::Inactive {
            api.plt_hook_register(
                0,
                0,
                getenv_symbol,
                my_getenv as *const (),
                &mut getenv_original,
            );
            api.plt_hook_register(
                0,
                0,
                property_find_symbol,
                my_system_property_find as *const (),
                &mut property_find_original,
            );
            api.plt_hook_register(
                0,
                0,
                property_read_callback_symbol,
                my_system_property_read_callback as *const (),
                &mut property_read_callback_original,
            );
            api.plt_hook_register(
                0,
                0,
                property_read_symbol,
                my_system_property_read as *const (),
                &mut property_read_original,
            );
            api.plt_hook_register(0, 0, dlsym_symbol, my_dlsym as *const (), &mut dlsym_original);

            if timezone_hook_mode == TimezoneOverrideKind::Value {
                api.plt_hook_register(
                    0,
                    0,
                    ucal_get_default_timezone_symbol,
                    my_ucal_get_default_time_zone as *const (),
                    &mut ucal_get_default_timezone_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_get_default_timezone_72_symbol,
                    my_ucal_get_default_time_zone as *const (),
                    &mut ucal_get_default_timezone_72_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_get_default_timezone_73_symbol,
                    my_ucal_get_default_time_zone as *const (),
                    &mut ucal_get_default_timezone_73_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_get_default_timezone_74_symbol,
                    my_ucal_get_default_time_zone as *const (),
                    &mut ucal_get_default_timezone_74_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_get_default_timezone_75_symbol,
                    my_ucal_get_default_time_zone as *const (),
                    &mut ucal_get_default_timezone_75_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_set_default_timezone_symbol,
                    my_ucal_set_default_time_zone as *const (),
                    &mut ucal_set_default_timezone_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_set_default_timezone_72_symbol,
                    my_ucal_set_default_time_zone as *const (),
                    &mut ucal_set_default_timezone_72_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_set_default_timezone_73_symbol,
                    my_ucal_set_default_time_zone as *const (),
                    &mut ucal_set_default_timezone_73_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_set_default_timezone_74_symbol,
                    my_ucal_set_default_time_zone as *const (),
                    &mut ucal_set_default_timezone_74_original,
                );
                api.plt_hook_register(
                    0,
                    0,
                    ucal_set_default_timezone_75_symbol,
                    my_ucal_set_default_time_zone as *const (),
                    &mut ucal_set_default_timezone_75_original,
                );
            }
        }

        let _ = api.plt_hook_commit();

        ORIGINAL_SYSTEM_PROPERTY_GET = if system_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(system_original))
        };
        ORIGINAL_PROPERTY_GET = if property_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(property_original))
        };
        ORIGINAL_GETENV = if getenv_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(getenv_original))
        };
        ORIGINAL_DLSYM = if dlsym_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(dlsym_original))
        };
        ORIGINAL_SYSTEM_PROPERTY_FIND = if property_find_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(property_find_original))
        };
        ORIGINAL_SYSTEM_PROPERTY_READ_CALLBACK = if property_read_callback_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(property_read_callback_original))
        };
        ORIGINAL_SYSTEM_PROPERTY_READ = if property_read_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(property_read_original))
        };
        let ucal_get_original = [
            ucal_get_default_timezone_original,
            ucal_get_default_timezone_72_original,
            ucal_get_default_timezone_73_original,
            ucal_get_default_timezone_74_original,
            ucal_get_default_timezone_75_original,
        ]
        .into_iter()
        .find(|ptr| !ptr.is_null())
        .unwrap_or(std::ptr::null());
        ORIGINAL_UCAL_GET_DEFAULT_TIME_ZONE = if ucal_get_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(ucal_get_original))
        };

        let ucal_set_original = [
            ucal_set_default_timezone_original,
            ucal_set_default_timezone_72_original,
            ucal_set_default_timezone_73_original,
            ucal_set_default_timezone_74_original,
            ucal_set_default_timezone_75_original,
        ]
        .into_iter()
        .find(|ptr| !ptr.is_null())
        .unwrap_or(std::ptr::null());
        ORIGINAL_UCAL_SET_DEFAULT_TIME_ZONE = if ucal_set_original.is_null() {
            None
        } else {
            Some(std::mem::transmute(ucal_set_original))
        };
    }

    Ok(())
}
