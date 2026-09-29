//! In-process Linux arm64 syscall layer for running original Android ELF
//! programs on Darwin (ADR 0012, phase P0).
//!
//! - [`loader`] maps a program (and its PT_INTERP) as Linux `binfmt_elf` does.
//! - [`xlate`] translates an original ELF file ahead of time (a pure
//!   function); [`cache`] stores translated files keyed by the original's
//!   sha256 and the translator version; [`xrt`] substitutes them when the
//!   guest opens an original, so text pages are file-backed and shared.
//! - [`patch`] rewrites code with no cache entry (run-time generated code,
//!   untranslated files) before it becomes executable.
//! - [`context`] and `trampoline.S` carry guest register state across the
//!   boundary; [`sys`] implements the syscalls with Linux semantics.

pub mod a64;
pub mod cache;
pub mod context;
pub mod diag;
pub mod elf;
pub mod errno;
pub mod hostcall;
pub mod hwcap;
pub mod loader;
pub mod patch;
pub mod sys;
pub mod vfs;
pub mod xlate;
pub mod xrt;
pub mod zip;

use std::ffi::CString;
use std::path::{Path, PathBuf};

pub struct RunOptions<'a> {
    pub root: &'a Path,
    /// `--path-map`: the guest filesystem view over `root`.
    pub path_map: Option<&'a Path>,
    /// Guest path of the program.
    pub program: &'a str,
    /// argv, including argv[0].
    pub argv: Vec<Vec<u8>>,
    pub envp: Vec<Vec<u8>>,
    /// AT_EXECFN: the filename the guest passed to execve; None: `program`.
    pub execfn: Option<Vec<u8>>,
    pub trace: bool,
    /// Translation cache directory; None: every file is rewritten at load
    /// time.
    pub cache: Option<PathBuf>,
    /// Bootstrap name of the binder host (`aim-binderd`); None: the
    /// binder device nodes do not exist.
    pub binder: Option<String>,
    pub identity: sys::cred::Identity,
    /// The process table directory (`by-pid`) the process belongs to;
    /// None: a new, private one (`sys/pidns.rs`).
    pub by_pid: Option<PathBuf>,
    /// State carried over the guest's last exec.
    pub state: sys::ExecState,
    /// The options describing this runtime (root, cache, tracing, ...),
    /// repeated when the guest execs another program.
    pub runtime_args: Vec<CString>,
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

fn trace_image(name: &str, i: &loader::Image) {
    let s = &i.stats;
    crate::diag!(
        "[linux-abi] {name} loaded at bias {:#x} from {}; load-time rewrites: {} svc, {} mrs/{} msr tpidr_el0, {} scs, {} ctr_el0 ({} brk fallbacks)",
        i.bias,
        i.source,
        s.svc,
        s.mrs_tp,
        s.msr_tp,
        s.scs,
        s.ctr,
        s.brk_fallback
    );
}

/// `linux-run --fork-child`: become the child of a guest fork in the
/// process that spawned this one (`sys::fork`). Only the runtime options
/// of `opts` are used; the process state comes from the parent. Returns
/// only on failure.
pub fn run_fork_child(opts: RunOptions) -> String {
    if let Err(e) = vfs::init(opts.root, opts.path_map) {
        return e;
    }
    sys::init_heap_window();
    sys::set_trace(opts.trace);
    xrt::init(Some(vfs::root()), opts.cache.clone());
    if let Some(name) = &opts.binder
        && let Err(e) = sys::init_binder(name)
    {
        return e;
    }
    sys::init_exec(opts.runtime_args);
    sys::become_fork_child()
}

/// Load `program` under `root` and run it on the current thread. Returns
/// only on a load error; the guest ends the process with exit_group.
pub fn run(opts: RunOptions) -> String {
    if let Err(e) = vfs::init(opts.root, opts.path_map) {
        return e;
    }
    sys::init_heap_window();
    sys::init_fds();
    sys::set_trace(opts.trace);
    xrt::init(Some(vfs::root()), opts.cache.clone());
    if let Some(name) = &opts.binder
        && let Err(e) = sys::init_binder(name)
    {
        return e;
    }
    let by_pid = match opts.by_pid.map_or_else(sys::new_pid_namespace, Ok) {
        Ok(d) => d,
        Err(e) => return format!("pid namespace: {e}"),
    };
    sys::cred::init(opts.identity, Some(by_pid));
    sys::init_exec(opts.runtime_args);
    context::init_thread();
    diag::install_signal_handlers();
    opts.state.apply();

    let resolved = match vfs::resolve(vfs::LINUX_AT_FDCWD, opts.program.as_bytes(), true) {
        Ok(r) => r,
        Err(e) => return format!("{}: cannot resolve (errno {e})", opts.program),
    };
    // A script started directly, as init execs otapreopt_slot, runs its
    // interpreter as binfmt_script would.
    let mut argv: Vec<std::ffi::CString> = opts
        .argv
        .iter()
        .map(|a| std::ffi::CString::new(a.clone()).unwrap_or_default())
        .collect();
    if argv.is_empty() {
        argv.push(std::ffi::CString::default());
    }
    let (resolved, argv) = match sys::interpret_script(resolved, opts.program.as_bytes(), argv) {
        Ok(r) => r,
        Err(e) => return format!("{}: cannot execute (errno {})", opts.program, -e),
    };
    let argv: Vec<Vec<u8>> = argv.into_iter().map(|a| a.into_bytes()).collect();
    let execfn = opts.execfn.as_deref().unwrap_or(opts.program.as_bytes());
    match load_program(&resolved, &argv, &opts.envp, execfn) {
        Ok((entry, sp)) => context::enter_guest(entry, sp),
        Err(e) => e,
    }
}

/// Load `program` (an ELF file; `#!` scripts are interpreted already) and
/// its interpreter, and build the initial stack as Linux's `binfmt_elf`
/// does. Returns the entry point and the initial stack pointer.
pub fn load_program(
    program: &vfs::Resolved,
    argv: &[Vec<u8>],
    envp: &[Vec<u8>],
    execfn: &[u8],
) -> Result<(u64, u64), String> {
    let image = loader::load_elf(&program.host, &program.guest)?;
    sys::set_exe(
        program.guest.clone(),
        program.host.to_string_lossy().into_owned(),
    );
    let (entry, interp_base) = match &image.interp {
        Some(interp) => {
            let r = vfs::resolve(vfs::LINUX_AT_FDCWD, interp.as_bytes(), true)
                .map_err(|e| format!("interpreter {interp}: cannot resolve (errno {e})"))?;
            let i = loader::load_elf(&r.host, &r.guest)?;
            if sys::tracing() {
                trace_image(&r.guest, &i);
            }
            (i.entry, i.bias)
        }
        None => (image.entry, 0),
    };
    if sys::tracing() {
        trace_image(&program.guest, &image);
    }
    sys::init_brk(image.end);
    let sp = loader::build_stack(&loader::StackInputs {
        argv,
        envp,
        execfn,
        program: &image,
        interp_base,
    })?;
    Ok((entry, sp))
}
