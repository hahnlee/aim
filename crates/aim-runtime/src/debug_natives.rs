//! `android.os.Debug` natives (frameworks/base/core/jni/android_os_Debug.cpp).
//!
//! Android reads these from procfs, the memtrack HAL, ion/dmabuf sysfs and
//! bionic's allocator. Memory facts come from the guest /proc the facade
//! serves (the device view's contract), and each source the device view does
//! not have answers the way the original does when its file or HAL is
//! missing: -1 for ion/dmabuf/GPU/CMA sizes, 0 for PSS/RSS, false for smaps
//! and backtrace dumps. The native heap is this process's malloc zone.

use crate::guest_procfs;
use crate::jni_env::{Env, native};
use jni_sys::{JNIEnv, jboolean, jint, jlong, jlongArray, jobject, jstring};
use std::ffi::c_void;

/// SysMemInfo's default tags in Debug.MEMINFO_* order, with "Zram:" at
/// MEMINFO_ZRAM_TOTAL (android_os_Debug_getMemInfo).
const MEMINFO_TAGS: [&str; 26] = [
    "MemTotal:",
    "MemFree:",
    "Buffers:",
    "Cached:",
    "Shmem:",
    "Slab:",
    "SReclaimable:",
    "SUnreclaim:",
    "SwapTotal:",
    "SwapFree:",
    "Zram:",
    "Mapped:",
    "VmallocUsed:",
    "PageTables:",
    "KernelStack:",
    "KReclaimable:",
    "Active:",
    "Inactive:",
    "Unevictable:",
    "MemAvailable:",
    "Active(anon):",
    "Inactive(anon):",
    "Active(file):",
    "Inactive(file):",
    "CmaTotal:",
    "CmaFree:",
];

/// A tag the kernel view does not print reads as 0, as in
/// SysMemInfo::ReadMemInfo; there is no zram device or /proc/vmallocinfo.
fn meminfo_values(contents: &str) -> [jlong; 26] {
    let mut values = [0; 26];
    for (value, tag) in values.iter_mut().zip(MEMINFO_TAGS) {
        *value = guest_procfs::kb_field(contents, tag).unwrap_or(0) as jlong;
    }
    values
}

unsafe extern "system" fn get_mem_info(env: *mut JNIEnv, _class: jobject, out: jlongArray) {
    let env = unsafe { Env::new(env) };
    if out.is_null() {
        env.throw(c"java/lang/NullPointerException", "out == null");
        return;
    }
    if env.array_length(out) < MEMINFO_TAGS.len() as jint {
        env.throw(c"java/lang/RuntimeException", "outLen < MEMINFO_COUNT");
        return;
    }
    let Some(contents) = guest_procfs::read("/proc/meminfo") else {
        env.throw(c"java/lang/RuntimeException", "SysMemInfo read failed");
        return;
    };
    env.set_long_array_region(out, 0, &meminfo_values(&contents));
}

#[repr(C)]
#[derive(Default)]
struct MallocStatistics {
    blocks_in_use: u32,
    size_in_use: usize,
    max_size_in_use: usize,
    size_allocated: usize,
}

unsafe extern "C" {
    fn malloc_zone_statistics(zone: *mut c_void, statistics: *mut MallocStatistics);
}

fn malloc_statistics() -> MallocStatistics {
    let mut statistics = MallocStatistics::default();
    // A null zone sums every malloc zone of the process.
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut statistics) };
    statistics
}

/// mallinfo().usmblks: the heap's mapped size.
unsafe extern "system" fn native_heap_size(_env: *mut JNIEnv, _class: jobject) -> jlong {
    malloc_statistics().size_allocated as jlong
}

/// mallinfo().uordblks: bytes in allocated blocks.
unsafe extern "system" fn native_heap_allocated(_env: *mut JNIEnv, _class: jobject) -> jlong {
    malloc_statistics().size_in_use as jlong
}

/// mallinfo().fordblks: bytes in free blocks.
unsafe extern "system" fn native_heap_free(_env: *mut JNIEnv, _class: jobject) -> jlong {
    let statistics = malloc_statistics();
    statistics
        .size_allocated
        .saturating_sub(statistics.size_in_use) as jlong
}

/// getDirtyPagesPid: the device view has no /proc/<pid>/smaps.
unsafe extern "system" fn memory_info_pid(
    _env: *mut JNIEnv,
    _class: jobject,
    _pid: jint,
    _info: jobject,
) -> jboolean {
    0
}

unsafe extern "system" fn memory_info_self(_env: *mut JNIEnv, _class: jobject, _info: jobject) {}

/// getPssPid: no memtrack HAL and no smaps_rollup, so 0.
unsafe extern "system" fn pss_pid(
    _env: *mut JNIEnv,
    _class: jobject,
    _pid: jint,
    _out: jlongArray,
    _memtrack: jlongArray,
) -> jlong {
    0
}

unsafe extern "system" fn pss_self(_env: *mut JNIEnv, _class: jobject) -> jlong {
    0
}

/// The guest status file of a process: the device view serves this
/// process's own as /proc/self/status.
fn status_rss_kb(pid: jint) -> Option<u64> {
    let path = if pid == unsafe { libc::getpid() } {
        "/proc/self/status".to_owned()
    } else {
        format!("/proc/{pid}/status")
    };
    guest_procfs::kb_field(&guest_procfs::read(&path)?, "VmRSS:")
}

/// getRssPid: VmRSS plus memtrack (none here); 0 when the status is missing.
unsafe extern "system" fn rss_pid(
    env: *mut JNIEnv,
    _class: jobject,
    pid: jint,
    memtrack: jlongArray,
) -> jlong {
    let env = unsafe { Env::new(env) };
    let Some(rss) = status_rss_kb(pid) else {
        return 0;
    };
    if !memtrack.is_null() {
        let length = env.array_length(memtrack).clamp(0, 4) as usize;
        env.set_long_array_region(memtrack, 0, &[0; 4][..length]);
    }
    rss as jlong
}

unsafe extern "system" fn rss_self(env: *mut JNIEnv, class: jobject) -> jlong {
    unsafe { rss_pid(env, class, libc::getpid(), std::ptr::null_mut()) }
}

/// libnativehelper's FileDescriptor check in openFile.
fn file_descriptor(env: Env, descriptor: jobject) -> Option<i32> {
    if descriptor.is_null() {
        env.throw(c"java/lang/NullPointerException", "fd == null");
        return None;
    }
    let fd = env.file_descriptor(descriptor);
    if fd < 0 {
        env.throw(c"java/lang/RuntimeException", "Invalid file descriptor");
        return None;
    }
    Some(fd)
}

/// No malloc debug: M_WRITE_MALLOC_LEAK_INFO_TO_FILE fails and nothing is
/// written.
unsafe extern "system" fn dump_native_heap(env: *mut JNIEnv, _class: jobject, descriptor: jobject) {
    let env = unsafe { Env::new(env) };
    let _ = file_descriptor(env, descriptor);
}

/// malloc_info(0, fp): the allocator's XML summary.
unsafe extern "system" fn dump_native_malloc_info(
    env: *mut JNIEnv,
    _class: jobject,
    descriptor: jobject,
) {
    let env = unsafe { Env::new(env) };
    let Some(fd) = file_descriptor(env, descriptor) else {
        return;
    };
    let statistics = malloc_statistics();
    let xml = format!(
        "<malloc version=\"darwin-1\">\n<heap nr=\"0\">\n<allocated>{}</allocated>\n\
         <mapped>{}</mapped>\n</heap>\n</malloc>\n",
        statistics.size_in_use, statistics.size_allocated
    );
    let _ = unsafe { libc::write(fd, xml.as_ptr().cast(), xml.len()) };
}

/// read_binder_stat: there is no /sys/kernel/debug/binder/stats.
unsafe extern "system" fn binder_stat(_env: *mut JNIEnv, _class: jobject) -> jint {
    -1
}

/// Binder object counts are kept by libbinder; the Darwin binder transport
/// does not export them.
unsafe extern "system" fn binder_object_count(_env: *mut JNIEnv, _class: jobject) -> jint {
    0
}

/// dump_backtrace_to_file_timeout without tombstoned/debuggerd.
unsafe extern "system" fn dump_backtrace(
    _env: *mut JNIEnv,
    _class: jobject,
    _pid: jint,
    _file: jstring,
    _timeout: jint,
) -> jboolean {
    0
}

/// GetUnreachableMemoryString when libmemunreachable cannot run.
unsafe extern "system" fn unreachable_memory(
    env: *mut JNIEnv,
    _class: jobject,
    _limit: jint,
    _contents: jboolean,
) -> jstring {
    let env = unsafe { Env::new(env) };
    env.new_string(c"Failed to get unreachable memory\n")
}

/// getFreeZramKb reads SwapFree from /proc/meminfo.
unsafe extern "system" fn zram_free(_env: *mut JNIEnv, _class: jobject) -> jlong {
    guest_procfs::meminfo_kb("SwapFree:").unwrap_or(0) as jlong
}

/// ion, dmabuf, GPU and CMA accounting are absent: -1.
unsafe extern "system" fn unsupported_kb(_env: *mut JNIEnv, _class: jobject) -> jlong {
    -1
}

/// getDmabufMappedSizeKb sums per-process dmabuf maps; none are mapped.
unsafe extern "system" fn dmabuf_mapped(_env: *mut JNIEnv, _class: jobject) -> jlong {
    0
}

/// isVmapStack: /proc/config.gz is absent.
unsafe extern "system" fn is_vmap_stack(_env: *mut JNIEnv, _class: jobject) -> jboolean {
    0
}

/// mallopt(M_LOG_STATS) is a bionic allocator option.
unsafe extern "system" fn log_allocator_stats(_env: *mut JNIEnv, _class: jobject) -> jboolean {
    0
}

/// register_android_os_Debug.
///
/// # Safety
/// `env` must be the calling thread's JNIEnv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aim_register_debug_natives(env: *mut JNIEnv) -> bool {
    let env = unsafe { Env::new(env) };
    let class = env.find_class(c"android/os/Debug");
    let methods = [
        native(
            c"getNativeHeapSize",
            c"()J",
            native_heap_size as *mut c_void,
        ),
        native(
            c"getNativeHeapAllocatedSize",
            c"()J",
            native_heap_allocated as *mut c_void,
        ),
        native(
            c"getNativeHeapFreeSize",
            c"()J",
            native_heap_free as *mut c_void,
        ),
        native(
            c"getMemoryInfo",
            c"(Landroid/os/Debug$MemoryInfo;)V",
            memory_info_self as *mut c_void,
        ),
        native(
            c"getMemoryInfo",
            c"(ILandroid/os/Debug$MemoryInfo;)Z",
            memory_info_pid as *mut c_void,
        ),
        native(c"getPss", c"()J", pss_self as *mut c_void),
        native(c"getPss", c"(I[J[J)J", pss_pid as *mut c_void),
        native(c"getRss", c"()J", rss_self as *mut c_void),
        native(c"getRss", c"(I[J)J", rss_pid as *mut c_void),
        native(c"getMemInfo", c"([J)V", get_mem_info as *mut c_void),
        native(
            c"dumpNativeHeap",
            c"(Ljava/io/FileDescriptor;)V",
            dump_native_heap as *mut c_void,
        ),
        native(
            c"dumpNativeMallocInfo",
            c"(Ljava/io/FileDescriptor;)V",
            dump_native_malloc_info as *mut c_void,
        ),
        native(
            c"getBinderSentTransactions",
            c"()I",
            binder_stat as *mut c_void,
        ),
        native(
            c"getBinderReceivedTransactions",
            c"()I",
            binder_stat as *mut c_void,
        ),
        native(
            c"getBinderLocalObjectCount",
            c"()I",
            binder_object_count as *mut c_void,
        ),
        native(
            c"getBinderProxyObjectCount",
            c"()I",
            binder_object_count as *mut c_void,
        ),
        native(
            c"getBinderDeathObjectCount",
            c"()I",
            binder_object_count as *mut c_void,
        ),
        native(
            c"dumpJavaBacktraceToFileTimeout",
            c"(ILjava/lang/String;I)Z",
            dump_backtrace as *mut c_void,
        ),
        native(
            c"dumpNativeBacktraceToFileTimeout",
            c"(ILjava/lang/String;I)Z",
            dump_backtrace as *mut c_void,
        ),
        native(
            c"getUnreachableMemory",
            c"(IZ)Ljava/lang/String;",
            unreachable_memory as *mut c_void,
        ),
        native(c"getZramFreeKb", c"()J", zram_free as *mut c_void),
        native(c"getIonHeapsSizeKb", c"()J", unsupported_kb as *mut c_void),
        native(
            c"getDmabufTotalExportedKb",
            c"()J",
            unsupported_kb as *mut c_void,
        ),
        native(
            c"getGpuPrivateMemoryKb",
            c"()J",
            unsupported_kb as *mut c_void,
        ),
        native(
            c"getDmabufHeapTotalExportedKb",
            c"()J",
            unsupported_kb as *mut c_void,
        ),
        native(c"getIonPoolsSizeKb", c"()J", unsupported_kb as *mut c_void),
        native(
            c"getDmabufMappedSizeKb",
            c"()J",
            dmabuf_mapped as *mut c_void,
        ),
        native(
            c"getDmabufHeapPoolsSizeKb",
            c"()J",
            unsupported_kb as *mut c_void,
        ),
        native(c"getGpuTotalUsageKb", c"()J", unsupported_kb as *mut c_void),
        native(c"isVmapStack", c"()Z", is_vmap_stack as *mut c_void),
        native(
            c"logAllocatorStats",
            c"()Z",
            log_allocator_stats as *mut c_void,
        ),
        native(
            c"getKernelCmaUsageKb",
            c"()J",
            unsupported_kb as *mut c_void,
        ),
    ];
    let registered = env.register_natives(class, &methods);
    env.delete_local_ref(class);
    registered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meminfo_follows_debug_slots() {
        let contents = "MemTotal:        8388608 kB\nMemFree:         4194304 kB\n\
                        MemAvailable:    4194304 kB\nSwapFree:              7 kB\n";
        let values = meminfo_values(contents);
        assert_eq!(values[0], 8388608); // MEMINFO_TOTAL
        assert_eq!(values[1], 4194304); // MEMINFO_FREE
        assert_eq!(values[9], 7); // MEMINFO_SWAP_FREE
        assert_eq!(values[10], 0); // MEMINFO_ZRAM_TOTAL: no zram device
        assert_eq!(values[19], 4194304); // MEMINFO_AVAILABLE
    }

    #[test]
    fn malloc_statistics_cover_live_allocations() {
        let block = vec![0u8; 1 << 20];
        let statistics = malloc_statistics();
        assert!(statistics.size_in_use >= block.len());
        assert!(statistics.size_allocated >= statistics.size_in_use);
    }
}
