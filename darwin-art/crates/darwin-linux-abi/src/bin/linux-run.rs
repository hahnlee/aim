//! `linux-run [--root DIR] [--trace] PROGRAM [ARGS...]`
//!
//! Runs an original Android arm64 ELF program in this process on the Linux
//! syscall layer. PROGRAM is a guest path, resolved under `--root`.

use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage: linux-run [--root DIR] [--trace] PROGRAM [ARGS...]");
    std::process::exit(2);
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut root = PathBuf::from("/");
    let mut trace = false;
    let program = loop {
        match args.next() {
            Some(a) if a == "--root" => {
                root = args.next().map(PathBuf::from).unwrap_or_else(|| usage())
            }
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
        program: &program,
        argv,
        envp: darwin_linux_abi::default_android_env(),
        trace,
    });
    eprintln!("linux-run: {err}");
    std::process::exit(127);
}
