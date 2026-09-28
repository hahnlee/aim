//! The original linker64 and linkerconfig of the pinned image, with and
//! without a translation cache, and page sharing of translated text across
//! processes. Skipped when the extracted image is absent.

use std::collections::BTreeMap;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const LINKER: &str = "/apex/com.android.runtime/bin/linker64";
const LINKERCONFIG: &str = "/apex/com.android.runtime/bin/linkerconfig";

/// The extracted pinned image (the `image` node of `cargo aim`).
fn root() -> Option<PathBuf> {
    aim_paths::original_image_with(LINKER.trim_start_matches('/'))
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = aim_linux_abi::cache::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn translate(cache: &Path, root: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_linux-translate"))
        .arg("--cache")
        .arg(cache)
        .arg(root)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "linux-translate failed:\n{text}");
    text
}

fn linux_run(cache: &Path, root: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_linux-run"));
    c.arg("--cache")
        .arg(cache)
        .arg("--root")
        .arg(root)
        .args(args);
    c
}

fn run_ok(cache: &Path, root: &Path, args: &[&str]) -> (String, String) {
    let out = linux_run(cache, root, args).output().unwrap();
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    );
    assert!(
        out.status.success(),
        "{args:?} failed: {:?}\n{so}\n{se}",
        out.status
    );
    (so, se)
}

fn check_linker_and_linkerconfig(cache: &Path, root: &Path) {
    let (out, err) = run_ok(cache, root, &[LINKER]);
    assert!(
        out.starts_with("Usage: /apex/com.android.runtime/bin/linker64"),
        "stdout: {out}\nstderr: {err}"
    );
    // linkerconfig prints its usage on stderr.
    let (_, err) = run_ok(cache, root, &[LINKER, LINKERCONFIG, "--help"]);
    assert!(err.contains("Usage : linkerconfig"), "{err}");
}

#[test]
fn linker64_runs_with_load_time_rewriting_and_from_the_cache() {
    let Some(ref root) = root() else { return };
    // Empty cache: every file is rewritten at load time.
    let empty = scratch("empty-cache");
    check_linker_and_linkerconfig(&empty, root);
    let (_, trace) = run_ok(&empty, root, &["--trace", LINKER]);
    assert!(trace.contains("from load-time rewrite"), "{trace}");

    // Warm cache: nothing is rewritten in memory.
    let cache = scratch("cache");
    let report = translate(&cache, root);
    assert!(report.contains("brk fallbacks:   0"), "{report}");
    check_linker_and_linkerconfig(&cache, root);
    let (_, trace) = run_ok(&cache, root, &["--trace", LINKER, LINKERCONFIG, "--help"]);
    assert!(trace.contains("from translation cache"), "{trace}");
    assert!(
        !trace.contains("[linux-abi] rewrote"),
        "no load-time rewriting with a warm cache:\n{trace}"
    );
    aim_linux_abi::cache::remove_tree(&empty).unwrap();
    aim_linux_abi::cache::remove_tree(&cache).unwrap();
}

// ---- page sharing ---------------------------------------------------------

#[repr(C)]
struct ProcRegionInfo {
    protection: u32,
    max_protection: u32,
    inheritance: u32,
    flags: u32,
    offset: u64,
    behavior: u32,
    user_wired_count: u32,
    user_tag: u32,
    pages_resident: u32,
    pages_shared_now_private: u32,
    pages_swapped_out: u32,
    pages_dirtied: u32,
    ref_count: u32,
    shadow_depth: u32,
    share_mode: u32,
    private_pages_resident: u32,
    shared_pages_resident: u32,
    obj_id: u32,
    depth: u32,
    address: u64,
    size: u64,
}

#[repr(C)]
struct ProcRegionWithPathInfo {
    prinfo: ProcRegionInfo,
    vip: libc::vnode_info_path,
}

const PROC_PIDREGIONPATHINFO: i32 = 8;
const SM_SHARED: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mapping {
    obj_id: u32,
    dev: u32,
    ino: u64,
    share_mode: u32,
    ref_count: u32,
    shared_resident: u32,
}

/// Executable regions of `pid` backed by files under `under`, by path.
fn exec_file_regions(pid: i32, under: &Path) -> BTreeMap<String, Mapping> {
    let mut out = BTreeMap::new();
    let mut addr = 0u64;
    loop {
        // SAFETY: proc_pidinfo fills our buffer.
        let mut ri: ProcRegionWithPathInfo = unsafe { std::mem::zeroed() };
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDREGIONPATHINFO,
                addr,
                (&mut ri as *mut ProcRegionWithPathInfo).cast(),
                std::mem::size_of::<ProcRegionWithPathInfo>() as i32,
            )
        };
        if n <= 0 {
            break;
        }
        let p = &ri.prinfo;
        // SAFETY: vip_path is a NUL-terminated buffer.
        let path = unsafe { std::ffi::CStr::from_ptr(ri.vip.vip_path.as_ptr() as *const _) }
            .to_string_lossy()
            .into_owned();
        if p.protection & libc::PROT_EXEC as u32 != 0 && Path::new(&path).starts_with(under) {
            let st = &ri.vip.vip_vi.vi_stat;
            out.insert(
                path,
                Mapping {
                    obj_id: p.obj_id,
                    dev: st.vst_dev,
                    ino: st.vst_ino,
                    share_mode: p.share_mode,
                    ref_count: p.ref_count,
                    shared_resident: p.shared_pages_resident,
                },
            );
        }
        addr = p.address + p.size;
    }
    out
}

/// Copy a guest path from the extracted image into `dst`, recreating guest
/// symlinks (which are absolute in the image's own namespace).
fn mirror(src: &Path, dst: &Path, guest: &str) {
    let rel = guest.trim_start_matches('/');
    let from = src.join(rel);
    let to = dst.join(rel);
    if to.symlink_metadata().is_ok() {
        return;
    }
    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
    let meta = from.symlink_metadata().unwrap();
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(&from).unwrap();
        symlink(&target, &to).unwrap();
        let t = target.to_string_lossy().into_owned();
        let next = if t.starts_with('/') {
            t
        } else {
            format!("{}/{t}", Path::new(guest).parent().unwrap().display())
        };
        mirror(src, dst, &next);
    } else {
        std::fs::copy(&from, &to).unwrap();
    }
}

struct Kill(Vec<Child>);
impl Drop for Kill {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Two processes run linkerconfig, which blocks opening a FIFO after the
/// guest linker mapped all its libraries. Every translated text mapping in
/// both is the same file, vnode and VM object, mapped shared.
#[test]
fn translated_text_pages_are_shared_across_processes() {
    let Some(ref src) = root() else { return };
    let root = scratch("share-root");
    for g in [
        LINKER,
        LINKERCONFIG,
        "/apex/com.android.runtime/lib64/libbase.so",
        "/apex/com.android.runtime/lib64/libc++.so",
        "/system/lib64/libc.so",
        "/system/lib64/libm.so",
        "/system/lib64/libdl.so",
        "/system/lib64/liblog.so",
        "/system/lib64/libc++.so",
    ] {
        mirror(src, &root, g);
    }
    for d in ["system/etc", "product", "system_ext", "out"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let fifo = root.join("system/etc/llndk.libraries.txt");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: creating a FIFO in our scratch root.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
    let cache = scratch("share-cache");
    translate(&cache, &root);

    let spawn = || {
        linux_run(&cache, &root, &[LINKER, LINKERCONFIG, "--target", "/out"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    };
    let kids = Kill(vec![spawn(), spawn()]);
    let cache_dir = cache.canonicalize().unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let maps = loop {
        let maps: Vec<_> = kids
            .0
            .iter()
            .map(|c| exec_file_regions(c.id() as i32, &cache_dir))
            .collect();
        // linker64, linkerconfig, libc, libm, libdl, liblog, libc++, libbase.
        // (The two libc++.so paths are one file, hence one entry.)
        if maps.iter().all(|m| m.len() >= 7) {
            break maps;
        }
        assert!(
            Instant::now() < deadline,
            "children did not block: {maps:#?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        maps[0].keys().collect::<Vec<_>>(),
        maps[1].keys().collect::<Vec<_>>()
    );
    for (path, a) in &maps[0] {
        let b = &maps[1][path];
        eprintln!("{path}: {a:?} / {b:?}");
        assert_eq!((a.dev, a.ino), (b.dev, b.ino), "{path}: same vnode");
        assert_eq!(a.obj_id, b.obj_id, "{path}: same VM object");
        assert_eq!(a.share_mode, SM_SHARED, "{path}: shared mapping");
        assert!(a.ref_count >= 2, "{path}: referenced by both processes");
    }
    drop(kids);
    aim_linux_abi::cache::remove_tree(&root).unwrap();
    aim_linux_abi::cache::remove_tree(&cache).unwrap();
}
