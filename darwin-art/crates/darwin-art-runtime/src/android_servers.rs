//! The system process's `libandroid_servers`: natives of the original
//! services.jar owners (frameworks/base/services/core/jni).
//!
//! SystemServer loads libandroid_servers before starting any service; here the
//! system process entry registers these before the first service class runs.
//! services.jar is not on the boot class path, so classes are resolved through
//! the system server's class loader.
//!
//! Each native answers as the Android implementation does on a kernel without
//! the feature it wraps, so the Java owner takes its documented fallback.

use crate::guest_procfs;
use crate::jni_env::{Env, native};
use jni_sys::{
    JNIEnv, JNINativeMethod, jboolean, jclass, jdouble, jint, jlong, jmethodID, jobject,
    jobjectArray, jstring, jvalue,
};
use std::ffi::c_void;

/// com_android_server_am_OomConnection.cpp: without the memevents BPF ring
/// buffer the listener cannot start, and OomConnectionThread logs and exits.
unsafe extern "system" fn oom_wait(env: *mut JNIEnv, _class: jclass) -> jobjectArray {
    let env = unsafe { Env::new(env) };
    env.throw(
        c"java/lang/RuntimeException",
        "Failed to initialize memevents listener",
    );
    std::ptr::null_mut()
}

/// com_android_server_am_LowMemDetector.cpp: no PSI monitors, so init fails
/// and LowMemDetector reports itself unavailable.
unsafe extern "system" fn low_mem_init(_env: *mut JNIEnv, _object: jobject) -> jint {
    -1
}

unsafe extern "system" fn low_mem_wait(_env: *mut JNIEnv, _object: jobject) -> jint {
    -1
}

/// com_android_server_am_Freezer.cpp: there is no cgroup v2 freezer.
unsafe extern "system" fn freezer_supported(_env: *mut JNIEnv, _class: jclass) -> jboolean {
    0
}

/// The binder driver has no BINDER_FREEZE; the ioctl's EINVAL surfaces as
/// Android's exception.
unsafe extern "system" fn freeze_binder(
    env: *mut JNIEnv,
    _class: jclass,
    _pid: jint,
    _freeze: jboolean,
    _timeout: jint,
) -> jint {
    let env = unsafe { Env::new(env) };
    env.throw(
        c"java/lang/RuntimeException",
        "Unable to freeze/unfreeze binder",
    );
    -libc::EINVAL
}

unsafe extern "system" fn binder_freeze_info(env: *mut JNIEnv, _class: jclass, _pid: jint) -> jint {
    let env = unsafe { Env::new(env) };
    env.throw(
        c"java/lang/RuntimeException",
        &std::io::Error::from_raw_os_error(libc::EINVAL).to_string(),
    );
    0
}

/// com_android_server_am_PhantomProcessList.cpp: no CgroupProcs attribute.
unsafe extern "system" fn cgroup_procs_path(
    env: *mut JNIEnv,
    _object: jobject,
    uid: jint,
    _pid: jint,
) -> jstring {
    let env = unsafe { Env::new(env) };
    if uid < 0 {
        env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("uid is negative: {uid}"),
        );
        return std::ptr::null_mut();
    }
    env.new_string(c"")
}

/// com_android_server_am_BatteryStatsService.cpp nativeWaitWakeup: there is
/// no suspend control service, so no wakeup is ever reported and the reader
/// thread stays blocked, as on a device whose callback never fires.
unsafe extern "system" fn wait_wakeup(env: *mut JNIEnv, _object: jobject, buffer: jobject) -> jint {
    let env = unsafe { Env::new(env) };
    if buffer.is_null() {
        env.throw(c"java/lang/NullPointerException", "null argument");
        return -1;
    }
    loop {
        std::thread::park();
    }
}

/// No Power Stats HAL: rail statistics are unavailable.
unsafe extern "system" fn rail_energy(env: *mut JNIEnv, _object: jobject, stats: jobject) {
    let env = unsafe { Env::new(env) };
    if stats.is_null() {
        env.throw(
            c"java/lang/NullPointerException",
            "The railstats jni input jobject jrailStats is null.",
        );
        return;
    }
    let class = env.object_class(stats);
    let method = env.method_id(class, c"setRailStatsAvailability", c"(Z)V");
    env.delete_local_ref(class);
    if !method.is_null() {
        env.call_void(stats, method, &[jvalue { z: 0 }]);
    }
}

/// com_android_server_am_CachedAppOptimizer.cpp. Without zram or a guest
/// /proc process list there is nothing to compact.
unsafe extern "system" fn compaction_noop(_env: *mut JNIEnv, _object: jobject) {}

unsafe extern "system" fn compact_process(
    _env: *mut JNIEnv,
    _object: jobject,
    _pid: jint,
    _flags: jint,
) {
}

unsafe extern "system" fn thread_cpu_time(_env: *mut JNIEnv, _object: jobject) -> jlong {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.tv_sec * 1_000_000_000 + time.tv_nsec
}

/// sysinfo freeswap / totalswap over the device view's /proc/meminfo; a
/// device without swap divides 0 by 0, as the original does.
unsafe extern "system" fn free_swap_percent(_env: *mut JNIEnv, _object: jobject) -> jdouble {
    let total = guest_procfs::meminfo_kb("SwapTotal:").unwrap_or(0);
    let free = guest_procfs::meminfo_kb("SwapFree:").unwrap_or(0);
    free as f64 / total as f64
}

/// No zram device (SysMemInfo::mem_zram_kb, mem_compacted_kb).
unsafe extern "system" fn zero_kb(_env: *mut JNIEnv, _object: jobject) -> jlong {
    0
}

/// com_android_server_utils_AnrTimer.cpp built without native support: the
/// Java AnrTimer keeps its handler-based timers.
unsafe extern "system" fn anr_supported(_env: *mut JNIEnv, _class: jclass) -> jboolean {
    0
}

unsafe extern "system" fn anr_create(
    _env: *mut JNIEnv,
    _timer: jobject,
    _name: jstring,
    _extend: jboolean,
    _freeze: jboolean,
) -> jlong {
    0
}

unsafe extern "system" fn anr_close(_env: *mut JNIEnv, _class: jclass, _pointer: jlong) -> jint {
    -1
}

unsafe extern "system" fn anr_start(
    _env: *mut JNIEnv,
    _class: jclass,
    _pointer: jlong,
    _pid: jint,
    _uid: jint,
    _timeout: jlong,
) -> jint {
    0
}

unsafe extern "system" fn anr_timer_op(
    _env: *mut JNIEnv,
    _class: jclass,
    _pointer: jlong,
    _timer: jint,
) -> jboolean {
    0
}

unsafe extern "system" fn anr_trace(
    _env: *mut JNIEnv,
    _class: jclass,
    _config: jobjectArray,
) -> jstring {
    std::ptr::null_mut()
}

unsafe extern "system" fn anr_dump(
    _env: *mut JNIEnv,
    _class: jclass,
    _pointer: jlong,
) -> jobjectArray {
    std::ptr::null_mut()
}

/// com_android_server_power_PowerManagerService.cpp without a power HAL or
/// suspend control: no HAL to initialize or boost, wake locks and autosuspend
/// have no /sys/power to write, and a mode change or forced suspend fails.
unsafe extern "system" fn power_init(_env: *mut JNIEnv, _object: jobject) {}

unsafe extern "system" fn power_suspend_blocker(_env: *mut JNIEnv, _class: jclass, _name: jstring) {
}

unsafe extern "system" fn power_auto_suspend(_env: *mut JNIEnv, _class: jclass, _enable: jboolean) {
}

unsafe extern "system" fn power_boost(
    _env: *mut JNIEnv,
    _class: jclass,
    _boost: jint,
    _duration: jint,
) {
}

unsafe extern "system" fn power_mode(
    _env: *mut JNIEnv,
    _class: jclass,
    _mode: jint,
    _enabled: jboolean,
) -> jboolean {
    0
}

unsafe extern "system" fn power_force_suspend(_env: *mut JNIEnv, _class: jclass) -> jboolean {
    0
}

fn natives(
    env: Env,
    loader: jobject,
    load: jmethodID,
    class: &str,
    methods: &[JNINativeMethod],
) -> bool {
    let class = env.load_class(loader, load, class);
    let registered = env.register_natives(class, methods);
    env.delete_local_ref(class);
    registered
}

fn register(env: Env, loader: jobject, load: jmethodID) -> bool {
    natives(
        env,
        loader,
        load,
        "com.android.server.am.OomConnection",
        &[native(
            c"waitOom",
            c"()[Landroid/os/OomKillRecord;",
            oom_wait as *mut c_void,
        )],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.am.LowMemDetector",
        &[
            native(c"init", c"()I", low_mem_init as *mut c_void),
            native(c"waitForPressure", c"()I", low_mem_wait as *mut c_void),
        ],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.am.Freezer",
        &[
            native(
                c"nativeIsFreezerSupported",
                c"()Z",
                freezer_supported as *mut c_void,
            ),
            native(
                c"nativeFreezeBinder",
                c"(IZI)I",
                freeze_binder as *mut c_void,
            ),
            native(
                c"nativeGetBinderFreezeInfo",
                c"(I)I",
                binder_freeze_info as *mut c_void,
            ),
        ],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.am.PhantomProcessList",
        &[native(
            c"nativeGetCgroupProcsPath",
            c"(II)Ljava/lang/String;",
            cgroup_procs_path as *mut c_void,
        )],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.am.BatteryStatsService",
        &[
            native(
                c"nativeWaitWakeup",
                c"(Ljava/nio/ByteBuffer;)I",
                wait_wakeup as *mut c_void,
            ),
            native(
                c"getRailEnergyPowerStats",
                c"(Lcom/android/internal/os/RailStats;)V",
                rail_energy as *mut c_void,
            ),
        ],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.am.CachedAppOptimizer",
        &[
            native(c"cancelCompaction", c"()V", compaction_noop as *mut c_void),
            native(c"threadCpuTimeNs", c"()J", thread_cpu_time as *mut c_void),
            native(
                c"getFreeSwapPercent",
                c"()D",
                free_swap_percent as *mut c_void,
            ),
            native(c"getUsedZramMemory", c"()J", zero_kb as *mut c_void),
            native(c"getMemoryFreedCompaction", c"()J", zero_kb as *mut c_void),
            native(c"compactSystem", c"()V", compaction_noop as *mut c_void),
            native(c"compactProcess", c"(II)V", compact_process as *mut c_void),
        ],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.power.PowerManagerService",
        &[
            native(c"nativeInit", c"()V", power_init as *mut c_void),
            native(
                c"nativeAcquireSuspendBlocker",
                c"(Ljava/lang/String;)V",
                power_suspend_blocker as *mut c_void,
            ),
            native(
                c"nativeReleaseSuspendBlocker",
                c"(Ljava/lang/String;)V",
                power_suspend_blocker as *mut c_void,
            ),
            native(
                c"nativeSetAutoSuspend",
                c"(Z)V",
                power_auto_suspend as *mut c_void,
            ),
            native(c"nativeSetPowerBoost", c"(II)V", power_boost as *mut c_void),
            native(c"nativeSetPowerMode", c"(IZ)Z", power_mode as *mut c_void),
            native(
                c"nativeForceSuspend",
                c"()Z",
                power_force_suspend as *mut c_void,
            ),
        ],
    ) && natives(
        env,
        loader,
        load,
        "com.android.server.utils.AnrTimer",
        &[
            native(
                c"nativeAnrTimerSupported",
                c"()Z",
                anr_supported as *mut c_void,
            ),
            native(
                c"nativeAnrTimerCreate",
                c"(Ljava/lang/String;ZZ)J",
                anr_create as *mut c_void,
            ),
            native(c"nativeAnrTimerClose", c"(J)I", anr_close as *mut c_void),
            native(c"nativeAnrTimerStart", c"(JIIJ)I", anr_start as *mut c_void),
            native(
                c"nativeAnrTimerCancel",
                c"(JI)Z",
                anr_timer_op as *mut c_void,
            ),
            native(
                c"nativeAnrTimerAccept",
                c"(JI)Z",
                anr_timer_op as *mut c_void,
            ),
            native(
                c"nativeAnrTimerDiscard",
                c"(JI)Z",
                anr_timer_op as *mut c_void,
            ),
            native(
                c"nativeAnrTimerRelease",
                c"(JI)Z",
                anr_timer_op as *mut c_void,
            ),
            native(
                c"nativeAnrTimerTrace",
                c"([Ljava/lang/String;)Ljava/lang/String;",
                anr_trace as *mut c_void,
            ),
            native(
                c"nativeAnrTimerDump",
                c"(J)[Ljava/lang/String;",
                anr_dump as *mut c_void,
            ),
        ],
    )
}

/// Register the services.jar natives. `loader` is the system server class
/// loader and `load` its `loadClass(String)`.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv; `loader` and `load` must be a
/// live ClassLoader reference and its `loadClass` method id.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_register_android_servers(
    env: *mut JNIEnv,
    loader: jobject,
    load: jmethodID,
) -> bool {
    if env.is_null() || loader.is_null() || load.is_null() {
        return false;
    }
    register(unsafe { Env::new(env) }, loader, load)
}
