//! APEX (`apex_payload.img` in a ZIP) and compressed APEX (`original_apex`
//! inside a `.capex` ZIP). The module name comes from `apex_manifest.pb`.
use crate::erofs::{self, Erofs};
use crate::ext4::{self, Ext4};
use crate::source::ReadAt;
use crate::tree::Tree;
use crate::zip::Archive;
use crate::{Result, invalid};

const MAX_ORIGINAL_APEX: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: String,
    pub version: u64,
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| invalid("truncated protobuf varint"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("overlong protobuf varint"))
}

/// Fields 1 (name) and 2 (version) of `ApexManifest`; others are skipped.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let mut at = 0usize;
    let mut name = None;
    let mut version = 0u64;
    while at < bytes.len() {
        let key = varint(bytes, &mut at)?;
        match key & 7 {
            0 => {
                let value = varint(bytes, &mut at)?;
                if key >> 3 == 2 {
                    version = value;
                }
            }
            1 => at += 8,
            2 => {
                let length = varint(bytes, &mut at)? as usize;
                let value = bytes
                    .get(at..at + length)
                    .ok_or_else(|| invalid("truncated protobuf field"))?;
                if key >> 3 == 1 {
                    name = Some(
                        String::from_utf8(value.to_vec())
                            .map_err(|_| invalid("APEX name is not UTF-8"))?,
                    );
                }
                at += length;
            }
            5 => at += 4,
            wire => return Err(invalid(format!("unsupported protobuf wire type {wire}"))),
        }
    }
    let name = name.ok_or_else(|| invalid("APEX manifest has no name"))?;
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(invalid(format!("invalid APEX name {name:?}")));
    }
    Ok(Manifest { name, version })
}

/// Decompress a `.capex` container's `original_apex`.
pub fn original_apex(capex: &dyn ReadAt) -> Result<Vec<u8>> {
    let archive = Archive::open(capex)?;
    let entry = archive
        .find(b"original_apex")
        .ok_or_else(|| invalid("CAPEX has no original_apex"))?;
    archive.read(entry, MAX_ORIGINAL_APEX)
}

/// Open an APEX and hand its manifest, payload filesystem and that
/// filesystem's format name to `visit`.
pub fn with_payload<T>(
    apex: &dyn ReadAt,
    visit: impl FnOnce(&Manifest, &dyn Tree, &'static str) -> Result<T>,
) -> Result<T> {
    let archive = Archive::open(apex)?;
    let manifest_entry = archive
        .find(b"apex_manifest.pb")
        .ok_or_else(|| invalid("APEX has no apex_manifest.pb"))?;
    let manifest = parse_manifest(&archive.read(manifest_entry, 1 << 20)?)?;
    let payload_entry = archive
        .find(b"apex_payload.img")
        .ok_or_else(|| invalid("APEX has no apex_payload.img"))?;
    let payload = archive.stored(payload_entry)?;
    if erofs::is_erofs(&payload) {
        visit(&manifest, &Erofs::open(&payload)?, "EROFS")
    } else if ext4::is_ext4(&payload) {
        visit(&manifest, &Ext4::open(&payload)?, "ext4")
    } else {
        Err(invalid(format!(
            "{}: apex_payload.img is neither EROFS nor ext4",
            manifest.name
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{lookup, read_file};
    use crate::zip::{DEFLATE, STORED, tests::build};

    fn manifest_bytes(name: &str, version: u8) -> Vec<u8> {
        let mut out = vec![0x0a, name.len() as u8];
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&[0x10, version, 0x2a, 1, b'x']);
        out
    }

    #[test]
    fn parses_manifest_fields() {
        let manifest = parse_manifest(&manifest_bytes("com.android.art", 7)).unwrap();
        assert_eq!(
            manifest,
            Manifest {
                name: "com.android.art".into(),
                version: 7
            }
        );
        assert!(parse_manifest(&manifest_bytes("../x", 1)).is_err());
        assert!(parse_manifest(&[0x0a, 5, b'a']).is_err());
    }

    #[test]
    fn opens_ext4_payloads_directly_and_through_capex() {
        let payload = crate::ext4::tests::build();
        let manifest = manifest_bytes("com.android.test", 3);
        let apex = build(&[
            ("apex_manifest.pb", DEFLATE, &manifest),
            ("apex_payload.img", STORED, &payload),
        ]);
        let capex = build(&[("original_apex", DEFLATE, &apex)]);
        let original = original_apex(&capex).unwrap();
        assert_eq!(original, apex);
        let (name, bytes) = with_payload(&original, |manifest, tree, format| {
            assert_eq!(format, "ext4");
            let id = lookup(tree, "/data").unwrap().unwrap();
            Ok((manifest.name.clone(), read_file(tree, id, 1 << 20)?.len()))
        })
        .unwrap();
        assert_eq!((name.as_str(), bytes), ("com.android.test", 3 * 1024 + 100));
        let deflated_payload = build(&[
            ("apex_manifest.pb", DEFLATE, &manifest),
            ("apex_payload.img", DEFLATE, &payload),
        ]);
        assert!(with_payload(&deflated_payload, |_, _, _| Ok(())).is_err());
    }
}
