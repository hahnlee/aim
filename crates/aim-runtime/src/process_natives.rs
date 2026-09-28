//! `android.os.Process` natives beyond the ones compat registers
//! (frameworks/base/core/jni/android_util_Process.cpp).
//!
//! Signals reach host processes, as the existing `sendSignal` does: an
//! Android pid is its host pid here. The device kernel has no cgroups, so
//! libprocessgroup's profiles have no usable controller and apply as no-ops
//! for a live process, a process group is the process itself, and freezing is
//! never supported. Thread scheduling groups use the Darwin tier adapter
//! behind `setThreadGroup`. Linux-only scheduler calls take the original's
//! non-Linux branch, and procfs reads use the device view's guest /proc.

use crate::guest_procfs;
use crate::jni_env::{Env, native};
use jni_sys::{
    JNIEnv, jboolean, jint, jintArray, jlong, jlongArray, jobject, jobjectArray, jstring, jvalue,
};
use std::ffi::c_void;

// SchedPolicy (processgroup/sched_policy.h).
const SP_DEFAULT: jint = -1;
const SP_FOREGROUND: jint = 1;
const SP_CNT: jint = 9;
// Android (Linux) errno values, as Java compares them with OsConstants.
const ANDROID_EPERM: i32 = 1;
const ANDROID_ESRCH: i32 = 3;
const ANDROID_EACCES: i32 = 13;
const ANDROID_EINVAL: i32 = 22;
const ANDROID_ENOSYS: i32 = 38;

unsafe extern "C" {
    // crates/aim-runtime/src/scheduling_groups.rs
    fn aim_thread_set_group(tid: i32, group: i32) -> i32;
}

/// signalExceptionForError.
fn error_exception(env: Env, error: i32, id: jint) {
    match error {
        ANDROID_EINVAL => env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("Invalid argument: {id}"),
        ),
        ANDROID_ESRCH => env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("Given thread {id} does not exist"),
        ),
        ANDROID_EPERM => env.throw(
            c"java/lang/SecurityException",
            &format!("No permission to modify given thread {id}"),
        ),
        _ => env.throw(c"java/lang/RuntimeException", "Unknown error"),
    }
}

/// signalExceptionForPriorityError.
fn priority_exception(env: Env, error: i32, id: jint) {
    if error == ANDROID_EACCES {
        env.throw(
            c"java/lang/SecurityException",
            &format!("No permission to set the priority of {id}"),
        );
    } else {
        error_exception(env, error, id);
    }
}

/// signalExceptionForGroupError.
fn group_exception(env: Env, error: i32, id: jint) {
    if error == ANDROID_EACCES {
        env.throw(
            c"java/lang/SecurityException",
            &format!("No permission to set the group of {id}"),
        );
    } else {
        error_exception(env, error, id);
    }
}

/// verifyGroup.
fn valid_group(env: Env, group: jint) -> bool {
    if !(SP_DEFAULT..SP_CNT).contains(&group) {
        error_exception(env, ANDROID_EINVAL, group);
        return false;
    }
    true
}

fn negative_uid(env: Env, uid: jint) -> bool {
    if uid < 0 {
        env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("uid is negative: {uid}"),
        );
        return true;
    }
    false
}

fn host_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Host errno as Android's for the values these natives report.
fn android_errno(error: i32) -> i32 {
    match error {
        libc::EPERM => ANDROID_EPERM,
        libc::ESRCH => ANDROID_ESRCH,
        libc::EACCES => ANDROID_EACCES,
        libc::EINVAL => ANDROID_EINVAL,
        _ => error,
    }
}

/// uid_from_pid succeeds for a live process.
fn process_exists(pid: jint) -> bool {
    pid > 0 && (unsafe { libc::kill(pid, 0) } == 0 || host_errno() == libc::EPERM)
}

unsafe extern "system" fn set_thread_group_and_cpuset(
    env: *mut JNIEnv,
    _class: jobject,
    tid: jint,
    group: jint,
) {
    let env = unsafe { Env::new(env) };
    if !valid_group(env, group) {
        return;
    }
    // There are no cpusets; the group's scheduling tier still applies.
    let error = unsafe { aim_thread_set_group(tid, group) };
    if error != 0 {
        group_exception(env, error, tid);
    }
}

unsafe extern "system" fn set_process_group(
    env: *mut JNIEnv,
    _class: jobject,
    pid: jint,
    group: jint,
) {
    let env = unsafe { Env::new(env) };
    if !valid_group(env, group) {
        return;
    }
    if group == SP_FOREGROUND {
        group_exception(env, ANDROID_EINVAL, pid);
        return;
    }
    if !process_exists(pid) {
        group_exception(env, ANDROID_ESRCH, pid);
    }
    // SetProcessProfilesCached: no cpuset controller, nothing to apply.
}

unsafe extern "system" fn set_process_frozen(
    env: *mut JNIEnv,
    _class: jobject,
    _pid: jint,
    uid: jint,
    _freeze: jboolean,
) {
    let env = unsafe { Env::new(env) };
    // No freezer controller: the Frozen/Unfrozen profiles have no action.
    negative_uid(env, uid);
}

unsafe extern "system" fn freeze_cgroup_uid(
    env: *mut JNIEnv,
    _class: jobject,
    uid: jint,
    _freeze: jboolean,
) {
    let env = unsafe { Env::new(env) };
    negative_uid(env, uid);
}

unsafe extern "system" fn create_process_group(
    env: *mut JNIEnv,
    _class: jobject,
    uid: jint,
    pid: jint,
) -> jint {
    let env = unsafe { Env::new(env) };
    if negative_uid(env, uid) {
        return 0;
    }
    if process_exists(pid) {
        0
    } else {
        -ANDROID_ESRCH
    }
}

unsafe extern "system" fn kill_process_group(
    env: *mut JNIEnv,
    _class: jobject,
    uid: jint,
    pid: jint,
) -> jint {
    let env = unsafe { Env::new(env) };
    if negative_uid(env, uid) || pid <= 0 {
        return -1;
    }
    // The group is the process; an already-exited one is killed.
    if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 || host_errno() == libc::ESRCH {
        0
    } else {
        -1
    }
}

unsafe extern "system" fn send_signal_to_process_group(
    env: *mut JNIEnv,
    _class: jobject,
    uid: jint,
    pid: jint,
    signal: jint,
) -> jboolean {
    let env = unsafe { Env::new(env) };
    if negative_uid(env, uid) || pid <= 0 {
        return 0;
    }
    (unsafe { libc::kill(pid, signal) } == 0) as jboolean
}

unsafe extern "system" fn remove_all_process_groups(_env: *mut JNIEnv, _class: jobject) {}

unsafe extern "system" fn send_signal_quiet(
    _env: *mut JNIEnv,
    _class: jobject,
    pid: jint,
    signal: jint,
) {
    if pid > 0 {
        unsafe { libc::kill(pid, signal) };
    }
}

unsafe extern "system" fn send_signal_throws(
    env: *mut JNIEnv,
    _class: jobject,
    pid: jint,
    signal: jint,
) {
    let env = unsafe { Env::new(env) };
    if pid <= 0 {
        env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("Invalid argument: pid({pid})"),
        );
        return;
    }
    if unsafe { libc::kill(pid, signal) } < 0 {
        let error = host_errno();
        if error == libc::ESRCH {
            env.throw(
                c"java/util/NoSuchElementException",
                &format!("Process with pid {pid} not found"),
            );
        } else {
            error_exception(env, android_errno(error), pid);
        }
    }
}

/// tgkill: only a process's main thread (tid == tgid) is addressable from
/// another process on Darwin.
unsafe extern "system" fn send_tg_signal_throws(
    env: *mut JNIEnv,
    class: jobject,
    tgid: jint,
    tid: jint,
    signal: jint,
) {
    if tgid <= 0 || tid <= 0 {
        let env = unsafe { Env::new(env) };
        env.throw(
            c"java/lang/IllegalArgumentException",
            &format!("Invalid argument: tgid({tid}), tid({tgid})"),
        );
        return;
    }
    if tid == tgid {
        unsafe { send_signal_throws(env, class, tgid, signal) };
    } else {
        error_exception(unsafe { Env::new(env) }, ANDROID_ENOSYS, tid);
    }
}

/// The host process identity is not the Android uid/gid; switching it is
/// refused as for an unprivileged caller.
unsafe extern "system" fn set_uid(env: *mut JNIEnv, _class: jobject, uid: jint) -> jint {
    if negative_uid(unsafe { Env::new(env) }, uid) {
        return 0;
    }
    ANDROID_EPERM
}

unsafe extern "system" fn set_gid(env: *mut JNIEnv, _class: jobject, gid: jint) -> jint {
    if gid < 0 {
        unsafe { Env::new(env) }.throw(
            c"java/lang/IllegalArgumentException",
            &format!("gid is negative: {gid}"),
        );
        return 0;
    }
    ANDROID_EPERM
}

/// sched_getscheduler/sched_setscheduler exist only on Linux.
unsafe extern "system" fn get_thread_scheduler(
    env: *mut JNIEnv,
    _class: jobject,
    tid: jint,
) -> jint {
    priority_exception(unsafe { Env::new(env) }, ANDROID_ENOSYS, tid);
    0
}

unsafe extern "system" fn set_thread_scheduler(
    env: *mut JNIEnv,
    _class: jobject,
    tid: jint,
    _policy: jint,
    _priority: jint,
) {
    priority_exception(unsafe { Env::new(env) }, ANDROID_ENOSYS, tid);
}

/// The device view's online CPUs (`0-7`) as a CPU list.
fn online_cpus() -> Vec<u32> {
    let text = guest_procfs::read("/sys/devices/system/cpu/online").unwrap_or_default();
    let mut cpus = Vec::new();
    for range in text.trim().split(',').filter(|range| !range.is_empty()) {
        let mut bounds = range.splitn(2, '-').map(|value| value.parse::<u32>());
        match (bounds.next(), bounds.next()) {
            (Some(Ok(start)), None) => cpus.push(start),
            (Some(Ok(start)), Some(Ok(end))) if start <= end => cpus.extend(start..=end),
            _ => {}
        }
    }
    cpus
}

/// sched_getaffinity: every thread may run on every online CPU.
unsafe extern "system" fn get_sched_affinity(
    env: *mut JNIEnv,
    _class: jobject,
    pid: jint,
) -> jlongArray {
    let env = unsafe { Env::new(env) };
    if pid < 0 || (pid != 0 && !process_exists(pid)) {
        error_exception(env, ANDROID_ESRCH, pid);
        return std::ptr::null_mut();
    }
    let cpus = online_cpus();
    let count = cpus.iter().max().map_or(0, |cpu| cpu + 1).max(1) as usize;
    let mut masks = vec![0i64; count.div_ceil(64)];
    for cpu in cpus {
        masks[cpu as usize / 64] |= 1 << (cpu % 64);
    }
    let array = env.new_long_array(masks.len() as jint);
    if !array.is_null() {
        env.set_long_array_region(array, 0, &masks);
    }
    array
}

/// get_exclusive_cpuset_cores: no exclusive cpusets.
unsafe extern "system" fn get_exclusive_cores(env: *mut JNIEnv, _class: jobject) -> jintArray {
    unsafe { Env::new(env) }.new_int_array(0)
}

/// opendir fails: the device view does not list guest /proc directories.
unsafe extern "system" fn get_pids(
    env: *mut JNIEnv,
    _class: jobject,
    file: jstring,
    _last: jintArray,
) -> jintArray {
    if file.is_null() {
        unsafe { Env::new(env) }.throw_null_pointer();
    }
    std::ptr::null_mut()
}

unsafe extern "system" fn get_pids_for_commands(
    env: *mut JNIEnv,
    _class: jobject,
    commands: jobjectArray,
) -> jintArray {
    if commands.is_null() {
        unsafe { Env::new(env) }.throw_null_pointer();
    }
    std::ptr::null_mut()
}

/// SmapsOrRollupPss fails without smaps.
unsafe extern "system" fn get_pss(_env: *mut JNIEnv, _class: jobject, _pid: jint) -> jlong {
    -1
}

/// The status file of a process in the device view.
fn status(pid: jint) -> Option<String> {
    if pid == unsafe { libc::getpid() } {
        guest_procfs::read("/proc/self/status")
    } else {
        guest_procfs::read(&format!("/proc/{pid}/status"))
    }
}

/// total, file, anon, swap and shmem RSS (kB) from /proc/<pid>/status.
fn rss_values(status: &str) -> [jlong; 5] {
    let mut values = [0; 5];
    for (value, tag) in
        values
            .iter_mut()
            .zip(["VmRSS:", "RssFile:", "RssAnon:", "VmSwap:", "RssShmem:"])
    {
        *value = guest_procfs::kb_field(status, tag).unwrap_or(0) as jlong;
    }
    values
}

unsafe extern "system" fn get_rss(env: *mut JNIEnv, _class: jobject, pid: jint) -> jlongArray {
    let env = unsafe { Env::new(env) };
    let values = status(pid).map_or([0; 5], |status| rss_values(&status));
    let array = env.new_long_array(5);
    if !array.is_null() {
        env.set_long_array_region(array, 0, &values);
    }
    array
}

/// pidfd_open is Linux-only: ErrnoException(ENOSYS), which Process reads as
/// "pidfd unsupported".
unsafe extern "system" fn pid_fd_open(
    env: *mut JNIEnv,
    _class: jobject,
    _pid: jint,
    _flags: jint,
) -> jint {
    let env = unsafe { Env::new(env) };
    let class = env.find_class(c"android/system/ErrnoException");
    let constructor = env.method_id(class, c"<init>", c"(Ljava/lang/String;I)V");
    let name = env.new_string(c"nativePidFdOpen");
    let exception = env.new_object(
        class,
        constructor,
        &[jvalue { l: name }, jvalue { i: ANDROID_ENOSYS }],
    );
    if !exception.is_null() {
        env.throw_object(exception);
    }
    env.delete_local_ref(name);
    env.delete_local_ref(class);
    -1
}

/// Register the android.os.Process natives compat does not.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aim_register_process_natives(env: *mut JNIEnv) -> bool {
    let env = unsafe { Env::new(env) };
    let class = env.find_class(c"android/os/Process");
    let methods = [
        native(
            c"setThreadGroupAndCpuset",
            c"(II)V",
            set_thread_group_and_cpuset as *mut c_void,
        ),
        native(
            c"setProcessGroup",
            c"(II)V",
            set_process_group as *mut c_void,
        ),
        native(
            c"setProcessFrozen",
            c"(IIZ)V",
            set_process_frozen as *mut c_void,
        ),
        native(
            c"freezeCgroupUid",
            c"(IZ)V",
            freeze_cgroup_uid as *mut c_void,
        ),
        native(
            c"createProcessGroup",
            c"(II)I",
            create_process_group as *mut c_void,
        ),
        native(
            c"killProcessGroup",
            c"(II)I",
            kill_process_group as *mut c_void,
        ),
        native(
            c"sendSignalToProcessGroup",
            c"(III)Z",
            send_signal_to_process_group as *mut c_void,
        ),
        native(
            c"removeAllProcessGroups",
            c"()V",
            remove_all_process_groups as *mut c_void,
        ),
        native(
            c"sendSignalQuiet",
            c"(II)V",
            send_signal_quiet as *mut c_void,
        ),
        native(
            c"sendSignalThrows",
            c"(II)V",
            send_signal_throws as *mut c_void,
        ),
        native(
            c"sendTgSignalThrows",
            c"(III)V",
            send_tg_signal_throws as *mut c_void,
        ),
        native(c"setUid", c"(I)I", set_uid as *mut c_void),
        native(c"setGid", c"(I)I", set_gid as *mut c_void),
        native(
            c"getThreadScheduler",
            c"(I)I",
            get_thread_scheduler as *mut c_void,
        ),
        native(
            c"setThreadScheduler",
            c"(III)V",
            set_thread_scheduler as *mut c_void,
        ),
        native(
            c"getSchedAffinity",
            c"(I)[J",
            get_sched_affinity as *mut c_void,
        ),
        native(
            c"getExclusiveCores",
            c"()[I",
            get_exclusive_cores as *mut c_void,
        ),
        native(
            c"getPids",
            c"(Ljava/lang/String;[I)[I",
            get_pids as *mut c_void,
        ),
        native(
            c"getPidsForCommands",
            c"([Ljava/lang/String;)[I",
            get_pids_for_commands as *mut c_void,
        ),
        native(c"getPss", c"(I)J", get_pss as *mut c_void),
        native(c"getRss", c"(I)[J", get_rss as *mut c_void),
        native(c"nativePidFdOpen", c"(II)I", pid_fd_open as *mut c_void),
    ];
    let registered = env.register_natives(class, &methods);
    env.delete_local_ref(class);
    registered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rss_fields_in_order() {
        let status =
            "Name:\tx\nVmRSS:\t262144 kB\nRssAnon:\t200 kB\nRssFile:\t50 kB\nVmSwap:\t0 kB\n";
        assert_eq!(rss_values(status), [262144, 50, 200, 0, 0]);
    }

    #[test]
    fn live_processes_exist() {
        assert!(process_exists(unsafe { libc::getpid() }));
        assert!(!process_exists(0));
        assert!(!process_exists(-5));
    }
}
