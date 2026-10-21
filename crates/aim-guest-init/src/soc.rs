//! The device's SoC, as a vendor's build.prop names it (`ro.soc.*`,
//! `Build.SOC_MANUFACTURER` and `Build.SOC_MODEL`). The device's SoC is
//! the Mac's (ADR 0010): its manufacturer is Apple and its model the
//! Mac's `machdep.cpu.brand_string`. The device's vendor build.prop sets
//! neither, so they come from here; a value the host cannot read is left
//! unset rather than invented.

use std::ffi::CStr;

/// `ro.soc.manufacturer` and `ro.soc.model`.
pub const MANUFACTURER_PROP: &str = "ro.soc.manufacturer";
pub const MODEL_PROP: &str = "ro.soc.model";

/// The properties for a Mac whose CPU brand string is `brand`.
pub fn of(brand: Option<&str>) -> Vec<(&'static str, String)> {
    match brand.map(str::trim).filter(|b| !b.is_empty()) {
        Some(brand) => vec![
            (MANUFACTURER_PROP, "Apple".to_string()),
            (MODEL_PROP, brand.to_string()),
        ],
        None => Vec::new(),
    }
}

/// This Mac's.
pub fn properties() -> Vec<(&'static str, String)> {
    of(brand_string().as_deref())
}

/// `sysctl machdep.cpu.brand_string`.
fn brand_string() -> Option<String> {
    let name: &CStr = c"machdep.cpu.brand_string";
    let mut buf = [0u8; 128];
    let mut size = buf.len();
    // SAFETY: a sysctl read into a local buffer of the size given.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if r != 0 {
        return None;
    }
    let value = CStr::from_bytes_until_nul(&buf[..size]).ok()?;
    value.to_str().ok().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_mac_or_nothing() {
        assert_eq!(
            of(Some("Apple M2 Pro")),
            vec![
                (MANUFACTURER_PROP, "Apple".to_string()),
                (MODEL_PROP, "Apple M2 Pro".to_string())
            ]
        );
        assert!(of(None).is_empty());
        assert!(of(Some("  ")).is_empty());
    }

    #[test]
    fn reads_this_macs_brand() {
        let brand = brand_string().expect("machdep.cpu.brand_string");
        assert!(!brand.is_empty() && !brand.contains('\0'), "{brand:?}");
    }
}
