//! `android.os.FileObserver$ObserverThread` natives
//! (frameworks/base/core/jni/android_util_FileObserver.cpp).
//!
//! This is the original's non-Linux build: there is no inotify, so `init`
//! returns -1, `observe` returns at once and watches are never added (their
//! descriptors stay -1). FileObserver callers therefore receive no events;
//! an inotify-equivalent over kqueue/FSEvents is separate work.

use crate::jni_env::{Env, native};
use jni_sys::{JNIEnv, jint, jintArray, jobject, jobjectArray};
use std::ffi::c_void;

unsafe extern "system" fn init(_env: *mut JNIEnv, _thread: jobject) -> jint {
    -1
}

unsafe extern "system" fn observe(_env: *mut JNIEnv, _thread: jobject, _fd: jint) {}

unsafe extern "system" fn start_watching(
    env: *mut JNIEnv,
    _thread: jobject,
    _fd: jint,
    _paths: jobjectArray,
    _mask: jint,
    descriptors: jintArray,
) {
    let env = unsafe { Env::new(env) };
    // ScopedIntArrayRW on a null array.
    if descriptors.is_null() {
        env.throw(
            c"java/lang/IllegalStateException",
            "Failed to get ScopedIntArrayRW",
        );
    }
}

unsafe extern "system" fn stop_watching(
    _env: *mut JNIEnv,
    _thread: jobject,
    _fd: jint,
    _descriptors: jintArray,
) {
}

/// register_android_os_FileObserver.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aim_register_file_observer_natives(env: *mut JNIEnv) -> bool {
    let env = unsafe { Env::new(env) };
    let class = env.find_class(c"android/os/FileObserver$ObserverThread");
    let registered = env.register_natives(
        class,
        &[
            native(c"init", c"()I", init as *mut c_void),
            native(c"observe", c"(I)V", observe as *mut c_void),
            native(
                c"startWatching",
                c"(I[Ljava/lang/String;I[I)V",
                start_watching as *mut c_void,
            ),
            native(c"stopWatching", c"(I[I)V", stop_watching as *mut c_void),
        ],
    );
    env.delete_local_ref(class);
    registered
}
