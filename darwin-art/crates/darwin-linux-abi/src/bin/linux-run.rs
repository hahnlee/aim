//! `linux-run [--root DIR] [--path-map FILE] [--cache DIR] [--binder NAME] [--seclabel LABEL] [--trace] PROGRAM [ARGS...]`
//!
//! Runs an original Android arm64 ELF program in this process on the Linux
//! syscall layer. PROGRAM is a guest path, resolved under `--root`.
//!
//! `--cache` is the translation cache directory (default
//! `~/Library/Caches/DarwinART/translated`, filled by `linux-translate`).
//! Files without an entry are rewritten at load time.
//!
//! `--path-map` is the guest filesystem view (its `root` and `rw` entries;
//! docs/guest-init-contract.md, section 2).
//!
//! `--binder` names the binder host (`darwin-binderd --service NAME`) that
//! backs `/dev/binder`, `/dev/hwbinder` and `/dev/vndbinder`. `--seclabel`
//! is the process's SELinux context.

use std::path::PathBuf;

fn usage() -> ! {
    eprintln!(
        "usage: linux-run [--root DIR] [--path-map FILE] [--cache DIR] [--binder NAME] [--seclabel LABEL] [--trace] PROGRAM [ARGS...]"
    );
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from("/");
    let mut cache = darwin_linux_abi::cache::Cache::default_dir();
    let mut trace = false;
    let mut binder = None;
    let mut path_map = None;
    let mut seclabel = None;
    let program = loop {
        match args.next() {
            Some(a) if a == "--root" => {
                root = args.next().map(PathBuf::from).unwrap_or_else(|| usage())
            }
            Some(a) if a == "--cache" => {
                cache = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage()))
            }
            Some(a) if a == "--path-map" => {
                path_map = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage()))
            }
            Some(a) if a == "--binder" => binder = Some(args.next().unwrap_or_else(|| usage())),
            Some(a) if a == "--seclabel" => seclabel = Some(args.next().unwrap_or_else(|| usage())),
            Some(a) if a == "--trace" => trace = true,
            Some(a) if a == "--help" || a == "-h" => usage(),
            Some(a) => break a,
            None => usage(),
        }
    };
    let mut argv = vec![program.clone()];
    argv.extend(args);
    let err = darwin_linux_abi::run(darwin_linux_abi::RunOptions {
        root: &root,
        path_map: path_map.as_deref(),
        program: &program,
        argv,
        envp: darwin_linux_abi::default_android_env(),
        trace,
        cache,
        binder,
        seclabel,
    });
    eprintln!("linux-run: {err}");
    std::process::exit(127);
}
