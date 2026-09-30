//! The device's SKU, as a bootloader reports it to init
//! (`androidboot.product.vendor.sku`). Macs differ in the sensors the
//! sensors HAL serves (docs/vendor-hals.md), so the SKU names the ones this
//! Mac has, and SystemConfig adds the features of
//! `/vendor/etc/permissions/sku_<sku>/`, as for a device vendor's SKUs.

use aim_hostcall::sensors::present;

/// The bootconfig key (without `androidboot.`).
pub const KEY: &str = "product.vendor.sku";

/// The SKU of a Mac whose sensors are `present` (`aim_hostcall::sensors::
/// present`), or none without a sensor.
pub fn of(present: u32) -> Option<&'static str> {
    match (present & present::LIGHT != 0, present & present::HINGE != 0) {
        (false, false) => None,
        (true, false) => Some("light"),
        (false, true) => Some("hinge"),
        (true, true) => Some("light_hinge"),
    }
}

/// This Mac's SKU: the sensors the HAL lists, read as it reads them.
pub fn host() -> Option<&'static str> {
    of(aim_host_sensors::read_sensors().present)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_sensors() {
        assert_eq!(of(0), None);
        assert_eq!(of(present::LIGHT), Some("light"));
        assert_eq!(of(present::HINGE), Some("hinge"));
        assert_eq!(of(present::LIGHT | present::HINGE), Some("light_hinge"));
    }
}
