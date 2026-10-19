//! The device's SKU, as a bootloader reports it to init
//! (`androidboot.product.vendor.sku`). Macs differ in the sensors the
//! sensors HAL serves (docs/vendor-hals.md) and in having a built-in camera
//! (docs/camera.md), so the SKU names the ones this Mac has, and
//! SystemConfig adds the features of `/vendor/etc/permissions/sku_<sku>/`,
//! as for a device vendor's SKUs.

use aim_hostcall::sensors::present;

/// The bootconfig key (without `androidboot.`).
pub const KEY: &str = "product.vendor.sku";

/// The SKU of a Mac whose sensors are `present` (`aim_hostcall::sensors::
/// present`) and that has a built-in camera or not (`front`), or none
/// without either.
pub fn of(present: u32, front: bool) -> Option<&'static str> {
    match (present & present::LIGHT != 0, present & present::HINGE != 0, front) {
        (false, false, false) => None,
        (true, false, false) => Some("light"),
        (false, true, false) => Some("hinge"),
        (true, true, false) => Some("light_hinge"),
        (false, false, true) => Some("front"),
        (true, false, true) => Some("light_front"),
        (false, true, true) => Some("hinge_front"),
        (true, true, true) => Some("light_hinge_front"),
    }
}

/// This Mac's SKU: the sensors the HAL lists, read as it reads them, and
/// whether the camera provider lists a built-in camera.
pub fn host() -> Option<&'static str> {
    of(aim_host_sensors::read_sensors().present, aim_host_camera::has_built_in())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_sensors_and_the_camera() {
        assert_eq!(of(0, false), None);
        assert_eq!(of(present::LIGHT, false), Some("light"));
        assert_eq!(of(present::HINGE, false), Some("hinge"));
        assert_eq!(of(present::LIGHT | present::HINGE, false), Some("light_hinge"));
        assert_eq!(of(0, true), Some("front"));
        assert_eq!(of(present::LIGHT | present::HINGE, true), Some("light_hinge_front"));
    }

    /// Each SKU's directory holds the features its name says, and only
    /// them (`image/overlay.toml`).
    #[test]
    fn the_overlay_declares_each_skus_features() {
        let overlay = include_str!("../../../image/overlay.toml");
        for present in [0, present::LIGHT, present::HINGE, present::LIGHT | present::HINGE] {
            for front in [false, true] {
                let Some(sku) = of(present, front) else { continue };
                let mut files: Vec<&str> = overlay
                    .lines()
                    .filter_map(|l| l.strip_prefix(&format!("path = \"/vendor/etc/permissions/sku_{sku}/")))
                    .map(|l| l.trim_end_matches('"'))
                    .collect();
                files.sort();
                let mut want = Vec::new();
                if front {
                    want.push("android.hardware.camera.front.xml");
                }
                if present & present::HINGE != 0 {
                    want.push("android.hardware.sensor.hinge_angle.xml");
                }
                if present & present::LIGHT != 0 {
                    want.push("android.hardware.sensor.light.xml");
                }
                assert_eq!(files, want, "sku_{sku}");
            }
        }
    }
}
