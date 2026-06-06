use std::{ffi::{CStr, CString}, ptr};

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
        OriginalNativeGetOneArg,
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

const PROPERTY_VALUE_MAX_LEN: usize = 91;

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
    // SAFETY: This runs during app specialization, before the target app starts executing
    // its own code. Mutating the process environment here avoids concurrent env access.
    unsafe {
        if timezone == "__DELETE__" {
            std::env::remove_var("TZ");
        } else {
            std::env::set_var("TZ", timezone);
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
pub fn hook_system_properties(api: &mut ZygiskApi<V4>, env: &mut EnvUnowned) -> anyhow::Result<()> {
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
    FAKE_PROPS.lock().unwrap().get(name).cloned()
}

unsafe fn copy_property_value(value: *mut libc::c_char, prop_value: &str) -> libc::c_int {
    let len = prop_value.len().min(PROPERTY_VALUE_MAX_LEN);
    unsafe {
        ptr::copy_nonoverlapping(prop_value.as_ptr().cast::<libc::c_char>(), value, len);
        value.add(len).write(0);
    }
    len as libc::c_int
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

    unsafe {
        if let Some(orig_fn) = ORIGINAL_PROPERTY_GET {
            return orig_fn(name, value, default_value);
        }
    }

    if default_value.is_null() {
        unsafe {
            value.write(0);
        }
        return 0;
    }

    let default_str = match unsafe { CStr::from_ptr(default_value).to_str() } {
        Ok(s) => s,
        Err(_) => {
            unsafe {
                value.write(0);
            }
            return 0;
        }
    };

    unsafe { copy_property_value(value, default_str) }
}

pub fn hook_native_property_get(api: &mut ZygiskApi<V4>) -> anyhow::Result<()> {
    let system_symbol = CString::new("__system_property_get").unwrap();
    let property_get_symbol = CString::new("property_get").unwrap();

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
    }

    Ok(())
}
