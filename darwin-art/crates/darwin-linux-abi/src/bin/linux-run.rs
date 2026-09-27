//! `linux-run [OPTIONS] PROGRAM [ARGS...]`
//!
//! Runs an original Android arm64 ELF program in this process on the Linux
//! syscall layer. PROGRAM is a guest path, resolved under `--root`.
//!
//! `--cache` is the translation cache directory (default
//! `~/Library/Caches/DarwinART/translated`, filled by `linux-translate`).
//! Files without an entry are rewritten at load time.
//!
//! `--path-map` is the guest filesystem view (its `root` and `rw` entries;
//! docs/guest-init-contract.md, section 2). `--binder` names the binder host
//! (`darwin-binderd --service NAME`) that backs `/dev/binder`,
//! `/dev/hwbinder` and `/dev/vndbinder`. `--gpu` is the directory of the
//! host GPU libraries (ANGLE's `libEGL.dylib` and `libGLESv2.dylib`) behind
//! the guest GLES driver (docs/gles-driver.md). `--display` is the socket
//! of the display server (`darwin-display`) behind the composer HAL
//! (docs/composer.md); its input devices are the guest's `/dev/input`
//! (docs/input.md).
//!
//! The options after `--identity` describe the process as darwin-guest-init
//! starts it (`docs/guest-init-contract.md`), or as a guest `execve`
//! re-executes `linux-run` with the state Linux keeps across exec.

use std::ffi::{CStr, CString, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use darwin_linux_abi::sys::{ExecState, cred::Identity};

const USAGE: &str = "usage: linux-run [OPTIONS] PROGRAM [ARGS...]

  --root DIR             guest root directory (default /)
  --path-map FILE        guest filesystem view over the root
  --cache DIR            translation cache directory
  --no-cache             rewrite every file at load time
  --binder NAME          binder host serving the binder device nodes
  --gpu DIR              host GPU libraries (ANGLE) for the GLES driver
  --display SOCKET       display server (darwin-display) for the composer
                         and input (/dev/input)
  --trace                log every syscall
  --stdio-null           the guest's stdin, stdout and stderr are /dev/null
                         (as init gives services); the layer's own messages
                         still go to the original stderr
  --diag-fd FD           the layer's messages go to FD (kept across exec)
  --identity FILE        the process's credentials (identity file); its
                         by-pid directory is FILE's directory + /by-pid
  --identity-text TEXT   the same, inline
  --seclabel LABEL       the process's SELinux context (overrides the
                         identity's seclabel)
  --by-pid DIR           the by-pid process table directory
  --inherit-env          the guest environment is linux-run's own
  --cwd DIR              guest working directory
  --sigmask HEX          blocked signals (bit n-1 = Linux signal n)
  --sigign HEX           ignored signals
  --personality HEX      personality(2) value
  --mounts TEXT          the process's own mounts (bind, tmpfs), one per line
  --exec EXECFN          PROGRAM is followed by the full argv (argv[0]
                         included) and EXECFN is AT_EXECFN, as after execve";

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn hex(v: OsString) -> u64 {
    u64::from_str_radix(&v.to_string_lossy(), 16).unwrap_or_else(|_| usage())
}

fn cstring(s: impl Into<Vec<u8>>) -> CString {
    CString::new(s).unwrap_or_else(|_| usage())
}

/// linux-run's own environment, verbatim and in order.
fn host_environment() -> Vec<Vec<u8>> {
    unsafe extern "C" {
        fn _NSGetEnviron() -> *const *const *const libc::c_char;
    }
    let mut out = Vec::new();
    // SAFETY: the process environment, a NULL-terminated array of C strings.
    unsafe {
        let mut p = *_NSGetEnviron();
        while !(*p).is_null() {
            out.push(CStr::from_ptr(*p).to_bytes().to_vec());
            p = p.add(1);
        }
    }
    out
}

/// Tells the Objective-C runtime that this executable forks while it has
/// threads, and that its children run on without exec: a guest `fork` is a
/// Darwin `fork` (zygote's children never exec). Without it, a child of a
/// multithreaded fork treats any two of its threads meeting in one class's
/// `+initialize` as a fork hazard and kills itself (the SIGKILL of
/// system_server seconds into its start, with no trace in the guest). The
/// runtime reads the section from the images it registers
/// (`DisableInitializeForkSafety` in objc4's `map_images_nolock`), which
/// are those with an `__objc_imageinfo` (an empty one: no classes).
#[used]
#[unsafe(link_section = "__DATA,__objc_fork_ok")]
static OBJC_FORK_OK: [u32; 2] = [0, 0];

#[used]
#[unsafe(link_section = "__DATA,__objc_imageinfo,regular,no_dead_strip")]
static OBJC_IMAGE_INFO: [u32; 2] = [0, 0];

fn main() {
    let mut args = std::env::args_os().skip(1);
    let mut root = PathBuf::from("/");
    let mut cache = darwin_linux_abi::cache::Cache::default_dir();
    let mut trace = false;
    let mut binder = None;
    let mut gpu: Option<PathBuf> = None;
    let mut display: Option<PathBuf> = None;
    let mut path_map: Option<PathBuf> = None;
    let mut seclabel = None;
    let mut identity = Identity::default();
    let mut by_pid = None;
    let mut inherit_env = false;
    let mut state = ExecState::default();
    let mut execfn = None;
    let mut runtime_args = Vec::new();
    let mut diag_fd = None;
    let program = loop {
        let Some(a) = args.next() else { usage() };
        let mut value = || args.next().unwrap_or_else(|| usage());
        match a.to_str().unwrap_or("") {
            "--root" => root = PathBuf::from(value()),
            "--cache" => cache = Some(PathBuf::from(value())),
            "--no-cache" => cache = None,
            "--path-map" => path_map = Some(PathBuf::from(value())),
            "--binder" => binder = Some(value().to_string_lossy().into_owned()),
            "--gpu" => gpu = Some(PathBuf::from(value())),
            "--display" => display = Some(PathBuf::from(value())),
            "--seclabel" => seclabel = Some(value().to_string_lossy().into_owned()),
            "--trace" => trace = true,
            "--stdio-null" => match darwin_linux_abi::diag::stdio_null() {
                Ok(fd) => diag_fd = Some(fd),
                Err(e) => {
                    darwin_linux_abi::diag!("linux-run: --stdio-null: {e}");
                    std::process::exit(127);
                }
            },
            "--diag-fd" => {
                let fd = value()
                    .to_string_lossy()
                    .parse()
                    .unwrap_or_else(|_| usage());
                darwin_linux_abi::diag::keep_log_fd(fd);
                diag_fd = Some(fd);
            }
            "--identity" => {
                let file = PathBuf::from(value());
                let text = std::fs::read_to_string(&file).unwrap_or_else(|e| {
                    darwin_linux_abi::diag!("linux-run: --identity {}: {e}", file.display());
                    std::process::exit(127);
                });
                identity = Identity::parse(&text).unwrap_or_else(|e| {
                    darwin_linux_abi::diag!("linux-run: --identity {}: {e}", file.display());
                    std::process::exit(127);
                });
                by_pid = file.parent().map(|d| d.join("by-pid"));
            }
            "--identity-text" => {
                identity = Identity::parse(&value().to_string_lossy()).unwrap_or_else(|e| {
                    darwin_linux_abi::diag!("linux-run: --identity-text: {e}");
                    std::process::exit(127);
                })
            }
            "--by-pid" => by_pid = Some(PathBuf::from(value())),
            "--inherit-env" => inherit_env = true,
            "--cwd" => state.cwd = Some(value().to_string_lossy().into_owned()),
            "--sigmask" => state.sigmask = hex(value()),
            "--sigign" => state.sigign = hex(value()),
            "--personality" => state.personality = hex(value()) as u32,
            "--mounts" => state.mounts = value().to_string_lossy().into_owned(),
            "--exec" => execfn = Some(value().into_vec()),
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => break a,
        }
    };
    runtime_args.extend([cstring("--root"), cstring(root.as_os_str().as_bytes())]);
    match &cache {
        Some(c) => runtime_args.extend([cstring("--cache"), cstring(c.as_os_str().as_bytes())]),
        None => runtime_args.push(cstring("--no-cache")),
    }
    if let Some(p) = &path_map {
        runtime_args.extend([cstring("--path-map"), cstring(p.as_os_str().as_bytes())]);
    }
    if let Some(b) = &binder {
        runtime_args.extend([cstring("--binder"), cstring(b.as_bytes())]);
    }
    if let Some(g) = &gpu {
        darwin_host_gpu::set_library_dir(g);
        runtime_args.extend([cstring("--gpu"), cstring(g.as_os_str().as_bytes())]);
    }
    if let Some(d) = &display {
        darwin_host_display::set_server(d);
        darwin_linux_abi::vfs::set_input_dir(&darwin_host_display::input::device_dir(d));
        runtime_args.extend([cstring("--display"), cstring(d.as_os_str().as_bytes())]);
    }
    if trace {
        runtime_args.push(cstring("--trace"));
    }
    if let Some(fd) = diag_fd {
        runtime_args.extend([cstring("--diag-fd"), cstring(fd.to_string())]);
    }
    if let Some(label) = seclabel {
        identity.seclabel = label;
    }
    let program = program.to_string_lossy().into_owned();
    let mut argv: Vec<Vec<u8>> = args.map(OsString::into_vec).collect();
    if execfn.is_none() {
        argv.insert(0, program.clone().into_bytes());
    }
    let envp = if inherit_env {
        host_environment()
    } else {
        darwin_linux_abi::default_android_env()
            .into_iter()
            .map(String::into_bytes)
            .collect()
    };
    let err = darwin_linux_abi::run(darwin_linux_abi::RunOptions {
        root: &root,
        path_map: path_map.as_deref(),
        program: &program,
        argv,
        envp,
        execfn,
        trace,
        cache,
        binder,
        identity,
        by_pid,
        state,
        runtime_args,
    });
    darwin_linux_abi::diag!("linux-run: {err}");
    std::process::exit(127);
}
