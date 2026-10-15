// Construct disposable malformed ZIPs from inputs, without editing any APK.
pub fn resource_apk(manifest: &[u8], resource: Option<(u16, u32, &[u8])>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut entries = vec![(
        "AndroidManifest.xml",
        0,
        manifest.len() as u32,
        crc32fast::hash(manifest),
        manifest,
    )];
    if let Some((method, size, payload)) = resource {
        entries.push((
            "resources.arsc",
            method,
            size,
            crc32fast::hash(payload) ^ 1,
            payload,
        ));
    }
    let count = entries.len() as u16;
    for (name, method, size, crc, payload) in entries {
        let offset = out.len() as u32;
        let padding = (4 - ((out.len() + 30 + name.len() + 4) % 4)) % 4;
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        for value in [20u16, 0, method, 0, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, payload.len() as u32, size] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [name.len() as u16, padding as u16 + 4] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&(padding as u16).to_le_bytes());
        out.extend(std::iter::repeat_n(0, padding));
        out.extend_from_slice(payload);
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        for value in [20u16, 20, 0, method, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, payload.len() as u32, size] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [name.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u32, offset] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        central.extend_from_slice(name.as_bytes());
    }
    let offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    for value in [0u16, 0, count, count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [central.len() as u32, offset] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
