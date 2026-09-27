//! Host-call ABI: how guest code reaches host implementations (ADR 0012,
//! boundary 2; `docs/host-call.md`).
//!
//! A guest component (a vendor HAL service built for `aarch64-linux-android`,
//! running on the Linux syscall layer) calls a host module the way a Wine
//! `.drv` calls its Unix side:
//!
//! ```text
//! x8 = SYSCALL_NR, x0 = module, x1 = function, x2 = args, x3 = args length
//! svc #0                                  -> x0 = result (>= 0) or -errno
//! ```
//!
//! - `args` points to the function's `#[repr(C)]` argument block in guest
//!   memory, which is host memory too: guest and host share one address
//!   space, so nothing is copied. The host checks that `len` is exactly the
//!   block's size before touching it.
//! - Register semantics are those of an AAPCS64 function call, not of a
//!   Linux syscall: x0 carries the result; x1-x17, v0-v7, v16-v31, the upper
//!   halves of v8-v15 and NZCV may be clobbered; everything else is kept.
//!   [`guest::call`] declares exactly that.
//! - Function 0 of every module ([`FN_VERSION`]) returns the module's
//!   version; module [`module::CORE`] reports [`ABI_VERSION`]. An unknown
//!   module or function returns `-ENOSYS`, which is also what a real Linux
//!   kernel answers for [`SYSCALL_NR`], so a guest can probe for the host.
//!
//! This crate is `no_std` and holds only the contract: numbers, argument
//! layouts, the guest-side call ([`guest`], aarch64 Linux/Android only) and
//! the shape of a host module ([`HostModule`]).

#![no_std]

/// Version of the calling convention itself (register use, [`FN_VERSION`],
/// error convention). Bumped only for incompatible changes.
pub const ABI_VERSION: u32 = 1;

/// The Linux syscall number that carries a host call: `"HC" << 16`, far
/// above any Linux arm64 syscall.
pub const SYSCALL_NR: u64 = 0x4843_0000;

/// Function 0 of every module: returns the module's version.
pub const FN_VERSION: u32 = 0;

/// Linux errno values used by the host-call layer and its modules.
pub mod errno {
    pub const ENODEV: i32 = 19;
    pub const EINVAL: i32 = 22;
    pub const ENOSYS: i32 = 38;
}

/// Module ids. They are fixed like syscall numbers: never reused, never
/// renumbered. A module evolves by bumping its version and adding
/// functions; an argument block never changes layout once shipped.
pub mod module {
    /// The host-call layer itself; its version is [`crate::ABI_VERSION`].
    pub const CORE: u32 = 0;
    /// Battery and power supply (the health HAL's host side).
    pub const HEALTH: u32 = 1;
}

/// Module [`module::HEALTH`]: the host's battery, for
/// `android.hardware.health`.
pub mod health {
    pub const VERSION: u32 = 1;

    /// Fill a [`Battery`]. Returns 0.
    pub const FN_BATTERY: u32 = 1;

    /// `android.hardware.health.BatteryStatus` values.
    pub mod status {
        pub const UNKNOWN: u32 = 1;
        pub const CHARGING: u32 = 2;
        pub const DISCHARGING: u32 = 3;
        pub const NOT_CHARGING: u32 = 4;
        pub const FULL: u32 = 5;
    }

    /// `android.hardware.health.BatteryHealth` values.
    pub mod condition {
        pub const UNKNOWN: u32 = 1;
        pub const GOOD: u32 = 2;
        pub const UNSPECIFIED_FAILURE: u32 = 6;
        pub const FAIR: u32 = 8;
        pub const NOT_AVAILABLE: u32 = 11;
    }

    /// Argument block of [`FN_BATTERY`] (output only). Units and signs
    /// follow `android.hardware.health.HealthInfo`; a value the host does
    /// not know is 0, and -1 for the two times.
    #[repr(C)]
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Battery {
        /// 0 when the host has no battery; the battery fields are then 0.
        pub present: u32,
        /// External power is connected.
        pub ac_online: u32,
        /// One of [`status`].
        pub status: u32,
        /// One of [`condition`].
        pub health: u32,
        pub level_percent: i32,
        pub voltage_millivolts: i32,
        /// Positive while charging, negative while discharging.
        pub current_microamps: i32,
        pub temperature_tenths_celsius: i32,
        pub cycle_count: i32,
        pub charge_counter_uah: i32,
        pub full_charge_uah: i32,
        pub full_charge_design_uah: i32,
        pub time_to_full_seconds: i64,
        pub time_to_empty_seconds: i64,
    }

    const _: () = assert!(core::mem::size_of::<Battery>() == 64);
}

/// A host module, as linked into the syscall layer's registry.
pub struct HostModule {
    pub id: u32,
    pub name: &'static str,
    /// Returned by [`FN_VERSION`], which the registry answers itself.
    pub version: u32,
    /// Runs function `func` (never [`FN_VERSION`]) with the guest's
    /// argument block. Returns a value >= 0 or a negative Linux errno.
    ///
    /// # Safety
    /// `args` is a guest address the host has not validated; `len` is the
    /// length the guest claims for it. The function must check `len` (see
    /// [`args_mut`]) before touching the block.
    pub call: unsafe fn(func: u32, args: u64, len: u64) -> i64,
}

/// The argument block of type `T` at guest address `args`, if `len` is its
/// exact size and `args` is non-null and aligned; `-EINVAL` otherwise.
///
/// # Safety
/// When the checks pass, `args` must point to `len` bytes of writable guest
/// memory not aliased for the returned lifetime (the calling thread's
/// argument block during its host call).
pub unsafe fn args_mut<'a, T>(args: u64, len: u64) -> Result<&'a mut T, i64> {
    let invalid = Err(-(errno::EINVAL as i64));
    if len != core::mem::size_of::<T>() as u64
        || args == 0
        || args % core::mem::align_of::<T>() as u64 != 0
    {
        return invalid;
    }
    // SAFETY: checked above; the caller vouches for the memory.
    Ok(unsafe { &mut *(args as *mut T) })
}

/// Guest side: the call instruction and typed wrappers.
#[cfg(all(
    target_arch = "aarch64",
    any(target_os = "android", target_os = "linux")
))]
pub mod guest {
    use super::*;

    /// A negative Linux errno from a host call.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Errno(pub i32);

    /// Raw host call.
    ///
    /// # Safety
    /// `args` must point to `len` bytes laid out as `module`'s function
    /// `func` expects.
    #[inline]
    pub unsafe fn call(module: u32, func: u32, args: *mut u8, len: usize) -> i64 {
        let ret: i64;
        // SAFETY: the host-call syscall, with AAPCS64 call clobbers.
        unsafe {
            core::arch::asm!(
                "svc #0",
                in("x8") SYSCALL_NR,
                inlateout("x0") module as u64 => ret,
                in("x1") func as u64,
                in("x2") args,
                in("x3") len,
                clobber_abi("C"),
            );
        }
        ret
    }

    fn check(ret: i64) -> Result<i64, Errno> {
        if ret < 0 {
            Err(Errno(-ret as i32))
        } else {
            Ok(ret)
        }
    }

    fn call_with<T>(module: u32, func: u32, args: &mut T) -> Result<i64, Errno> {
        // SAFETY: T is the argument block the function documents.
        check(unsafe {
            call(
                module,
                func,
                (args as *mut T).cast(),
                core::mem::size_of::<T>(),
            )
        })
    }

    /// Version of `module`; `Err(Errno(ENOSYS))` when the module (or the
    /// host-call layer) is absent.
    pub fn version(module: u32) -> Result<u32, Errno> {
        // SAFETY: FN_VERSION takes no argument block.
        check(unsafe { call(module, FN_VERSION, core::ptr::null_mut(), 0) }).map(|v| v as u32)
    }

    /// The host's battery.
    pub fn battery() -> Result<health::Battery, Errno> {
        let mut b = health::Battery::default();
        call_with(module::HEALTH, health::FN_BATTERY, &mut b)?;
        Ok(b)
    }
}
