//! The SystemServer exception on the pinned image's services.jar: the
//! patched jar leaves ClipboardService unstarted and everything else as it
//! was, with valid checksums.

use aim_android_image::dex::{self, Dex};
use aim_android_image::system_server::patch_services_jar;
use sha1::{Digest, Sha1};

const JAR: &str = "system/framework/services.jar";

fn entries(path: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    // The dex entries are stored: read them through their local headers
    // and check their CRCs.
    let mut out = Vec::new();
    let mut at = 0;
    while bytes[at..at + 4] == [0x50, 0x4b, 0x03, 0x04] {
        let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
        let (method, size, crc) = (u16_at(at + 8), u32_at(at + 18), u32_at(at + 14));
        let (name_len, extra_len) = (u16_at(at + 26), u16_at(at + 28));
        let name = String::from_utf8_lossy(&bytes[at + 30..at + 30 + name_len]).into_owned();
        let data = at + 30 + name_len + extra_len;
        if method == 0 {
            let body = bytes[data..data + size].to_vec();
            assert_eq!(crc32fast::hash(&body) as usize, crc, "{name}: CRC");
            out.push((name, body));
        }
        at = data + size;
    }
    out
}

#[test]
fn clipboard_is_not_started() {
    let Some(image) = aim_paths::original_image_with(JAR) else {
        return;
    };
    let out = std::env::temp_dir().join(format!("services-{}.jar", std::process::id()));
    patch_services_jar(
        &image.join(JAR),
        &out,
        &["com.android.server.clipboard.ClipboardService".into()],
    )
    .unwrap();
    let original = entries(&image.join(JAR));
    let patched = entries(&out);
    std::fs::remove_file(&out).unwrap();
    assert_eq!(original.len(), patched.len());
    let mut changed = 0;
    for ((name, before), (_, after)) in original.iter().zip(&patched) {
        if before == after {
            continue;
        }
        changed += 1;
        assert_eq!(before.len(), after.len());
        // Checksums as ART's verifier checks them.
        assert_eq!(
            &after[12..32],
            Sha1::digest(&after[32..]).as_slice(),
            "{name}"
        );
        let mut adler = adler2::Adler32::new();
        adler.write_slice(&after[12..]);
        assert_eq!(after[8..12], adler.checksum().to_le_bytes(), "{name}");
        // Exactly the five units of const-class + invoke-virtual differ.
        let differing: Vec<usize> = (32..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        let (first, last) = (differing[0], *differing.last().unwrap());
        assert!(
            last - first < 10,
            "{name}: edits span {first:#x}..{last:#x}"
        );
        assert!(after[first..=last].iter().all(|b| *b == 0));
        let dex = Dex::parse(after).unwrap();
        let class = dex.class("Lcom/android/server/SystemServer;").unwrap();
        let code = dex.methods_named(class, "startOtherServices").unwrap();
        let units = dex::units(after, &code[0]).unwrap();
        let mut at = 0;
        while at < units.len() {
            if units[at] & 0xff == 0x1c {
                let loaded = dex.type_name(u32::from(units[at + 1])).unwrap();
                assert_ne!(loaded, "Lcom/android/server/clipboard/ClipboardService;");
            }
            at += dex::instruction_units(&units, at).unwrap();
        }
    }
    assert_eq!(changed, 1);
}
