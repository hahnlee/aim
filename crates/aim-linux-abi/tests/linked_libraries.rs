//! `linux-run` starts for every guest process and fork child, and dyld
//! loads and initializes each library it links in every one of them
//! (docs/fork.md, "Cost"). Host modules open the frameworks they use on
//! first use (`aim_hostcall::dylib!`), so it links only libSystem and the
//! libiconv Rust's standard library links. ld keeps a library that only
//! dead-stripped code referenced, so this reads the binary's load commands.

const LC_REQ_DYLD: u32 = 0x8000_0000;
const LC_LOAD_DYLIB: u32 = 0xc;
const LC_LOAD_WEAK_DYLIB: u32 = 0x18 | LC_REQ_DYLD;
const LC_REEXPORT_DYLIB: u32 = 0x1f | LC_REQ_DYLD;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x23 | LC_REQ_DYLD;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

/// The install names of the libraries a thin 64-bit Mach-O loads.
fn linked_libraries(bytes: &[u8]) -> Vec<String> {
    assert_eq!(u32_at(bytes, 0), 0xfeed_facf, "not a thin 64-bit Mach-O");
    let commands = u32_at(bytes, 16);
    let mut at = 32;
    let mut names = Vec::new();
    for _ in 0..commands {
        let (cmd, size) = (u32_at(bytes, at), u32_at(bytes, at + 4) as usize);
        if matches!(
            cmd,
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB
        ) {
            let name = &bytes[at + u32_at(bytes, at + 8) as usize..at + size];
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            names.push(String::from_utf8_lossy(&name[..end]).into_owned());
        }
        at += size;
    }
    names
}

#[test]
fn linux_run_links_only_libsystem_and_libiconv() {
    let bytes = std::fs::read(env!("CARGO_BIN_EXE_linux-run")).unwrap();
    assert_eq!(
        linked_libraries(&bytes),
        ["/usr/lib/libSystem.B.dylib", "/usr/lib/libiconv.2.dylib"]
    );
}
