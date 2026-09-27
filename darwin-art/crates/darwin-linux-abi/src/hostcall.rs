//! Host-call dispatch (ADR 0012 boundary 2; `docs/host-call.md`).
//!
//! A guest reaches a host module with the Linux syscall
//! [`darwin_hostcall::SYSCALL_NR`]. The syscall entry sends it here on its
//! own path: guest x19/x20 are kept on the guest stack, the host stack is
//! switched to, and [`linux_abi_hostcall`] runs as an ordinary C function,
//! so only the AAPCS64 callee-saved registers survive (the contract the
//! guest wrapper declares). Traced runs and `brk` fallback sites come
//! through [`crate::sys`] with the full register save instead.
//!
//! Modules are Rust crates linked into the syscall layer; [`MODULES`] is the
//! registry, indexed by module id.

use darwin_hostcall::{ABI_VERSION, FN_VERSION, HostModule, errno, module};

static CORE: HostModule = HostModule {
    id: module::CORE,
    name: "core",
    version: ABI_VERSION,
    call: core_call,
};

unsafe fn core_call(_func: u32, _args: u64, _len: u64) -> i64 {
    -(errno::ENOSYS as i64)
}

/// Every host module, at the index of its id.
static MODULES: &[&HostModule] = &[
    &CORE,
    &darwin_host_health::MODULE,
    &darwin_host_gpu::MODULE,
    &darwin_host_display::MODULE,
    &darwin_host_sensors::THERMAL,
    &darwin_host_sensors::SENSORS,
    &darwin_host_location::MODULE,
    &darwin_host_audio::MODULE,
    &darwin_host_bluetooth::MODULE,
];

const _: () = {
    // The syscall entry matches the number with one `movz ..., lsl #16`.
    assert!(darwin_hostcall::SYSCALL_NR & !0xffff_0000 == 0);
    let mut i = 0;
    while i < MODULES.len() {
        assert!(MODULES[i].id as usize == i, "MODULES must be indexed by id");
        i += 1;
    }
};

/// Run a host call. Returns a value >= 0 or a negative Linux errno.
pub fn call(module: u64, func: u64, args: u64, len: u64) -> i64 {
    let (Ok(module), Ok(func)) = (u32::try_from(module), u32::try_from(func)) else {
        return -(errno::ENOSYS as i64);
    };
    let Some(m) = MODULES.get(module as usize) else {
        return -(errno::ENOSYS as i64);
    };
    if func == FN_VERSION {
        return m.version as i64;
    }
    // SAFETY: the module validates the argument block (HostModule::call).
    unsafe { (m.call)(func, args, len) }
}

/// Name of a module, for tracing.
pub fn module_name(module: u64) -> &'static str {
    MODULES.get(module as usize).map_or("?", |m| m.name)
}

/// Entry from the host-call path of `trampoline.S`, on the host stack.
#[unsafe(no_mangle)]
extern "C" fn linux_abi_hostcall(module: u64, func: u64, args: u64, len: u64) -> i64 {
    match std::panic::catch_unwind(|| call(module, func, args, len)) {
        Ok(r) => r,
        Err(_) => {
            crate::diag!(
                "[linux-abi] panic in host module {} function {func}; aborting",
                module_name(module)
            );
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_unknown_calls() {
        assert_eq!(call(0, 0, 0, 0), ABI_VERSION as i64);
        assert_eq!(
            call(module::HEALTH as u64, 0, 0, 0),
            darwin_hostcall::health::VERSION as i64
        );
        let enosys = -(errno::ENOSYS as i64);
        assert_eq!(call(0, 1, 0, 0), enosys);
        assert_eq!(call(MODULES.len() as u64, 0, 0, 0), enosys);
        assert_eq!(call(1 << 32, 0, 0, 0), enosys);
        assert_eq!(
            call(module::BLUETOOTH as u64, 0, 0, 0),
            darwin_hostcall::bluetooth::VERSION as i64
        );
    }
}
