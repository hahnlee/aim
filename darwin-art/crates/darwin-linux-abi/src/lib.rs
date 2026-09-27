//! In-process Linux arm64 syscall layer for running original Android ELF
//! programs on Darwin (ADR 0012, phase P0).
//!
//! - [`loader`] maps a program (and its PT_INTERP) as Linux `binfmt_elf` does.
//! - [`patch`] rewrites `svc #0` and `TPIDR_EL0` accesses before code runs.
//! - [`context`] and `trampoline.S` carry guest register state across the
//!   boundary; [`sys`] implements the syscalls with Linux semantics.

pub mod context;
pub mod diag;
pub mod elf;
pub mod errno;
pub mod hwcap;
pub mod loader;
pub mod patch;
pub mod sys;
pub mod vfs;

use std::path::Path;

pub struct RunOptions<'a> {
    pub root: &'a Path,
    /// Guest path of the program.
    pub program: &'a str,
    /// argv, including argv[0].
    pub argv: Vec<String>,
    pub envp: Vec<String>,
    pub trace: bool,
}

/// The environment a freshly started Android process sees from init.
pub fn default_android_env() -> Vec<String> {
    [
        "PATH=/product/bin:/apex/com.android.runtime/bin:/apex/com.android.art/bin:/system_ext/bin:/system/bin:/system/xbin:/odm/bin:/vendor/bin:/vendor/xbin",
        "ANDROID_ROOT=/system",
        "ANDROID_DATA=/data",
        "ANDROID_STORAGE=/storage",
        "ANDROID_ART_ROOT=/apex/com.android.art",
        "ANDROID_I18N_ROOT=/apex/com.android.i18n",
        "ANDROID_TZDATA_ROOT=/apex/com.android.tzdata",
        "ANDROID_ASSETS=/system/app",
        "HOME=/",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Load `program` under `root` and run it on the current thread. Returns
/// only on a load error; the guest ends the process with exit_group.
pub fn run(opts: RunOptions) -> String {
    if let Err(e) = vfs::init(opts.root) {
        return format!("--root {}: {e}", opts.root.display());
    }
    sys::set_trace(opts.trace);
    context::init_thread();
    diag::install_signal_handlers();

    let resolved = match vfs::resolve(vfs::LINUX_AT_FDCWD, opts.program.as_bytes(), true) {
        Ok(r) => r,
        Err(e) => return format!("{}: cannot resolve (errno {e})", opts.program),
    };
    let program = match loader::load_elf(&resolved.host, &resolved.guest) {
        Ok(i) => i,
        Err(e) => return e,
    };
    sys::set_exe(
        resolved.guest.clone(),
        resolved.host.to_string_lossy().into_owned(),
    );
    let (entry, interp_base) = match &program.interp {
        Some(interp) => {
            let r = match vfs::resolve(vfs::LINUX_AT_FDCWD, interp.as_bytes(), true) {
                Ok(r) => r,
                Err(e) => return format!("interpreter {interp}: cannot resolve (errno {e})"),
            };
            match loader::load_elf(&r.host, &r.guest) {
                Ok(i) => (i.entry, i.bias),
                Err(e) => return e,
            }
        }
        None => (program.entry, 0),
    };
    if opts.trace {
        eprintln!(
            "[linux-abi] {} loaded at bias {:#x}, entry {:#x}; rewrote {} svc, {} mrs/{} msr tpidr_el0 ({} brk fallbacks)",
            resolved.guest,
            program.bias,
            entry,
            program.stats.svc,
            program.stats.mrs_tp,
            program.stats.msr_tp,
            program.stats.brk_fallback
        );
    }
    sys::init_brk(program.end);
    let sp = match loader::build_stack(&loader::StackInputs {
        argv: &opts.argv,
        envp: &opts.envp,
        execfn: &resolved.guest,
        program: &program,
        interp_base,
    }) {
        Ok(sp) => sp,
        Err(e) => return e,
    };
    context::enter_guest(entry, sp)
}
