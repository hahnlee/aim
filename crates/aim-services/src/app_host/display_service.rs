//! `display`: DisplayManager for the app host's one display, the built-in
//! one SurfaceFlinger reports, as `LocalDisplayAdapter` and
//! `LogicalDisplay` describe it at `android-16.0.0_r1` (`DisplayInfo`'s
//! parcel form), with no decor taking from apps' area, and the stable
//! display size `PersistentDataStore` keeps
//! (`/data/system/display-manager-state.xml`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use aim_android_xml::{Element, Node, TEXT};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_hardware_display_idisplaymanager as dm;

use super::display::Display;
use super::{AppHost, reply};

/// `Display.FLAG_*` of a built-in display.
const FLAG_SUPPORTS_PROTECTED_BUFFERS: i32 = 1 << 0;
const FLAG_SECURE: i32 = 1 << 1;
const FLAG_TRUSTED: i32 = 1 << 7;
const FLAG_ROTATES_WITH_CONTENT: i32 = 1 << 14;
/// `Display.TYPE_INTERNAL`, `Display.STATE_ON`, `ColorSpace.Named.SRGB`.
const TYPE_INTERNAL: i32 = 1;
const STATE_ON: i32 = 2;
const SRGB: i32 = 0;
/// `PowerManager.BRIGHTNESS_*`: the setting's integer range.
const BRIGHTNESS_ON: f32 = 255.0;
const BRIGHTNESS_OFF: f32 = 1.0;

/// What the display info reads besides SurfaceFlinger's display.
pub struct Resources {
    /// `display_manager_built_in_display_name`, localized.
    pub name: String,
    pub density_dpi: i32,
    /// The settings' brightness range and default, from
    /// `config_screenBrightnessSetting{Minimum,Maximum,Default}`, and
    /// `config_screenBrightnessDimFloat`.
    pub brightness: (i32, i32, i32),
    pub brightness_dim: f32,
}

/// `DisplayInfo` of the default display.
pub struct DisplayInfo<'a> {
    pub display: &'a Display,
    pub resources: &'a Resources,
}

/// `BrightnessSynchronizer.brightnessIntToFloat`.
fn brightness(value: i32) -> f32 {
    (value as f32 - BRIGHTNESS_OFF) / (BRIGHTNESS_ON - BRIGHTNESS_OFF)
}

impl WriteParcelable for DisplayInfo<'_> {
    fn write_to(&self, p: &mut Parcel) {
        let d = self.display;
        let mode = d.active();
        let (w, h) = (d.width, d.height);
        p.write_i32(0); // layer stack
        let secure = if d.secure {
            FLAG_SECURE | FLAG_SUPPORTS_PROTECTED_BUFFERS
        } else {
            0
        };
        p.write_i32(secure | FLAG_TRUSTED | FLAG_ROTATES_WITH_CONTENT);
        p.write_i32(TYPE_INTERNAL);
        p.write_i32(0); // display id
        p.write_i32(0); // display group
        // `writeParcelable(DisplayAddress.Physical)`.
        p.write_string16(Some("android.view.DisplayAddress$Physical"));
        p.write_i64(d.id);
        p.write_string16(None); // no device product info
        p.write_string8(Some(&self.resources.name));
        for v in [w, h, w.min(h), w.min(h), w.max(h), w.max(h), w, h] {
            p.write_i32(v); // app, smallest and largest nominal app, logical
        }
        p.write_i32(-1); // no cutout
        p.write_i32(0); // rotation
        p.write_i32(d.active_mode);
        p.write_f32(d.render_frame_rate);
        p.write_bool(d.has_arr_support);
        p.write_string16(Some("android.view.FrameRateCategoryRate"));
        p.write_f32(d.frame_rate_category_rate.0);
        p.write_f32(d.frame_rate_category_rate.1);
        p.write_i32(d.supported_refresh_rates.len() as i32);
        for r in &d.supported_refresh_rates {
            p.write_f32(*r);
        }
        p.write_i32(d.active_mode); // the default mode
        p.write_i32(-1); // no user-preferred mode
        for _ in 0..2 {
            // The supported modes, and those apps may use.
            p.write_i32(d.modes.len() as i32);
            for m in &d.modes {
                p.write_i32(m.id);
                p.write_i32(m.width);
                p.write_i32(m.height);
                p.write_f32(m.peak_refresh_rate);
                p.write_f32(m.vsync_rate);
                p.write_bool(false); // not synthetic
                p.write_i32(0); // no alternative refresh rates
                p.write_i32(m.supported_hdr_types.len() as i32);
                for t in &m.supported_hdr_types {
                    p.write_i32(*t);
                }
            }
        }
        p.write_i32(d.active_color_mode);
        p.write_i32(d.supported_color_modes.len() as i32);
        for c in &d.supported_color_modes {
            p.write_i32(*c);
        }
        p.write_string16(Some("android.view.Display$HdrCapabilities"));
        p.write_i32(d.hdr_types.len() as i32);
        for t in &d.hdr_types {
            p.write_i32(*t);
        }
        p.write_f32(d.hdr_luminance.0);
        p.write_f32(d.hdr_luminance.1);
        p.write_f32(d.hdr_luminance.2);
        p.write_bool(false); // not forced to SDR
        p.write_bool(false); // no minimal post-processing
        p.write_i32(self.resources.density_dpi);
        p.write_f32(mode.x_dpi);
        p.write_f32(mode.y_dpi);
        p.write_i64(mode.app_vsync_offset);
        p.write_i64(mode.presentation_deadline);
        p.write_i32(STATE_ON);
        p.write_i32(STATE_ON); // committed
        p.write_i32(0); // owner uid
        p.write_string8(None); // owner package
        p.write_string8(Some(&format!("local:{}", d.id)));
        p.write_i32(0); // remove mode
        // `DisplayManagerService.getDisplayInfoForFrameRateOverride`: the
        // render rate when no override applies.
        p.write_f32(d.render_frame_rate);
        let (min, max, default) = self.resources.brightness;
        p.write_f32(brightness(min));
        p.write_f32(brightness(max));
        p.write_f32(brightness(default));
        p.write_f32(self.resources.brightness_dim);
        p.write_i32(1);
        p.write_i32(0); // `RoundedCorners.NO_ROUNDED_CORNERS`
        p.write_i32(0); // no user-disabled HDR types
        p.write_i32(d.install_orientation);
        // `DisplayShape.createDefaultDisplayShape`: the display's rectangle.
        p.write_i32(1);
        p.write_string8(Some(&format!("M0,0 L{w},0 L{w},{h} L0,{h} Z")));
        p.write_i32(w);
        p.write_i32(h);
        p.write_f32(1.0); // physical pixel display size ratio
        p.write_i32(0); // rotation
        p.write_i32(0); // offset x
        p.write_i32(0); // offset y
        p.write_f32(1.0); // scale
        p.write_i32(0); // no layout-limited refresh rate
        p.write_f32(f32::NAN); // HDR/SDR ratio: not HDR
        p.write_i32(0); // no thermal refresh rate throttling
        p.write_string8(Some("default")); // thermal brightness throttling data
        p.write_bool(true); // hosts tasks
    }
}

/// `WifiDisplayStatus()`: unavailable, not scanning, not connected, no
/// displays, and a default `WifiDisplaySessionInfo` (a client, session 0,
/// empty group, passphrase and address).
struct NoWifiDisplay;

impl WriteParcelable for NoWifiDisplay {
    fn write_to(&self, p: &mut Parcel) {
        for _ in 0..3 {
            p.write_i32(0); // feature, scan and active display states
        }
        p.write_i32(0); // no active display
        p.write_i32(0); // no displays
        p.write_i32(1);
        p.write_i32(0);
        for _ in 0..3 {
            p.write_string16(Some(""));
        }
    }
}

/// `PersistentDataStore`'s file under the guest's `/data`.
const STATE_XML: &str = "system/display-manager-state.xml";

/// A `Point`.
struct Point(i32, i32);

impl WriteParcelable for Point {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0);
        p.write_i32(self.1);
    }
}

/// The text of `e`'s child `name` (`nextText`).
fn child_text<'a>(e: &'a Element, name: &str) -> Option<&'a str> {
    e.children()
        .find(|c| c.name == name)?
        .content
        .iter()
        .find_map(|n| match n {
            Node::Token(TEXT, Some(text)) => Some(text.as_str()),
            _ => None,
        })
}

/// `StableDeviceValues.loadFromXml`: the persisted width and height, 0
/// where one is not a number.
fn stable_values(root: &Element) -> (i32, i32) {
    let Some(values) = root.children().find(|e| e.name == "stable-device-values") else {
        return (0, 0);
    };
    let int = |name: &str| {
        child_text(values, name)
            .and_then(|t| t.parse().ok())
            .unwrap_or(0)
    };
    (int("stable-display-width"), int("stable-display-height"))
}

/// `root` with its stable values set to `size` (`saveToXml`), or the
/// document `PersistentDataStore` writes from an empty state.
fn with_stable_values(root: Option<Element>, size: (i32, i32)) -> Element {
    let element = |name: &str, content: Vec<Node>| Element {
        name: name.into(),
        attrs: Vec::new(),
        content,
    };
    let text = |name: &str, value: String| {
        Node::Element(element(name, vec![Node::Token(TEXT, Some(value))]))
    };
    let mut root = root.unwrap_or_else(|| {
        let empty = |name: &str| Node::Element(element(name, Vec::new()));
        element(
            "display-manager-state",
            vec![
                empty("remembered-wifi-displays"),
                empty("display-states"),
                empty("stable-device-values"),
                empty("brightness-configurations"),
                text("brightness-nits-for-default-display", "-1.0".into()),
            ],
        )
    });
    let values = vec![
        text("stable-display-width", size.0.to_string()),
        text("stable-display-height", size.1.to_string()),
    ];
    match root.content.iter_mut().find_map(|n| match n {
        Node::Element(e) if e.name == "stable-device-values" => Some(e),
        _ => None,
    }) {
        Some(e) => e.content = values,
        None => root
            .content
            .push(Node::Element(element("stable-device-values", values))),
    }
    root
}

/// Writes `root` to the store's file, as the system's, through a new file
/// (`AtomicFile`).
fn write_state(data: &Path, root: &Element) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write(root).map_err(std::io::Error::other)?;
    let path = data.join(STATE_XML);
    let new = data.join(format!("{STATE_XML}.new"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        },
    )?;
    std::fs::rename(&new, path)
}

/// `display`.
pub struct DisplayManager {
    host: Arc<AppHost>,
    callbacks: Mutex<Vec<(Binder, i64)>>,
    /// `mStableDisplaySize`, once loaded.
    stable: Mutex<Option<(i32, i32)>>,
}

impl DisplayManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        DisplayManager {
            host,
            callbacks: Mutex::new(Vec::new()),
            stable: Mutex::new(None),
        }
    }

    /// `mStableDisplaySize`: the persisted size
    /// (`loadStableDisplayValuesLocked`), else the configured one, else
    /// the default display's natural size
    /// (`recordStableDisplayStatsIfNeededLocked`); one not persisted is
    /// (`setStableDisplaySizeLocked`). A store that cannot be read is
    /// empty, as `PersistentDataStore.load` clears it.
    fn stable_display_size(&self) -> Result<(i32, i32), Exception> {
        let mut stable = self.stable.lock().unwrap();
        if let Some(size) = *stable {
            return Ok(size);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let data = self.host.data()?;
        let path = data.join(STATE_XML);
        let root = match std::fs::read(&path) {
            Ok(bytes) => aim_android_xml::read(&bytes)
                .inspect_err(|e| {
                    eprintln!("app host: Failed to load display manager persistent store data: {e}")
                })
                .ok()
                .filter(|r| r.name == "display-manager-state"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(state(format!("{}: {e}", path.display()))),
        };
        let persisted = root.as_ref().map_or((0, 0), stable_values);
        let size = if persisted.0 > 0 && persisted.1 > 0 {
            persisted
        } else {
            let packages = &self.host.inputs()?.packages;
            let integer = |name: &str| {
                packages
                    .framework_integer(name)
                    .ok_or_else(|| state(format!("no {name}")))
            };
            let configured = (
                integer("config_stableDeviceDisplayWidth")?,
                integer("config_stableDeviceDisplayHeight")?,
            );
            let size = if configured.0 > 0 && configured.1 > 0 {
                configured
            } else {
                let display = self.host.display()?;
                (display.0.width, display.0.height)
            };
            write_state(&data, &with_stable_values(root, size))
                .map_err(|e| state(format!("{}: {e}", path.display())))?;
            size
        };
        *stable = Some(size);
        Ok(size)
    }
}

impl Service for DisplayManager {
    fn descriptor(&self) -> &str {
        dm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            dm::GET_DISPLAY_INFO => {
                let args = dm::GetDisplayInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let display = match self.host.display() {
                    Ok(d) => d,
                    Err(e) => return reply(Err(e)),
                };
                let info = (args.display_id == 0).then(|| DisplayInfo {
                    display: &display.0,
                    resources: &display.1,
                });
                dm::write_get_display_info_reply(&mut p, info.as_ref());
            }
            dm::GET_DISPLAY_IDS => {
                dm::GetDisplayIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                dm::write_get_display_ids_reply(&mut p, &Some(vec![0]));
            }
            dm::REGISTER_CALLBACK_WITH_EVENT_MASK => {
                let args = dm::RegisterCallbackWithEventMask::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // The display never changes, so no event is sent yet.
                if let Some(callback) = args.callback {
                    self.callbacks
                        .lock()
                        .unwrap()
                        .push((callback, args.events_mask));
                }
                dm::write_register_callback_with_event_mask_reply(&mut p);
            }
            dm::GET_PREFERRED_WIDE_GAMUT_COLOR_SPACE_ID => {
                dm::GetPreferredWideGamutColorSpaceId::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                dm::write_get_preferred_wide_gamut_color_space_id_reply(&mut p, SRGB);
            }
            dm::GET_OVERLAY_SUPPORT => {
                dm::GetOverlaySupport::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let support = match Display::overlay_support(&self.host.process) {
                    Ok(support) => support,
                    Err(e) => {
                        return reply(Err(aim_binder_host::parcel::Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            e,
                        )));
                    }
                };
                p.write_no_exception();
                p.write_raw(&support, &[]);
            }
            // `getWifiDisplayStatusInternal`: the default status without a
            // Wi-Fi display adapter (`registerWifiDisplayAdapterLocked`:
            // `config_enableWifiDisplay` or `persist.debug.wfd.enable`), whose
            // Wi-Fi P2P is not modelled.
            dm::GET_WIFI_DISPLAY_STATUS => {
                dm::GetWifiDisplayStatus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let configured = self.host.inputs().and_then(|inputs| {
                    inputs
                        .packages
                        .framework_bool("config_enableWifiDisplay")
                        .ok_or_else(|| {
                            Exception::new(EX_ILLEGAL_STATE, "no config_enableWifiDisplay")
                        })
                });
                let forced = self
                    .host
                    .property("persist.debug.wfd.enable")
                    .and_then(|v| super::activity::parse_int(&v))
                    == Some(1);
                match configured {
                    Ok(true) => {
                        return reply(Err(self
                            .host
                            .unimplemented_named("display", "a Wi-Fi display adapter (#1658)")));
                    }
                    Ok(false) if forced => {
                        return reply(Err(self
                            .host
                            .unimplemented_named("display", "a Wi-Fi display adapter (#1658)")));
                    }
                    Ok(false) => {}
                    Err(e) => return reply(Err(e)),
                }
                dm::write_get_wifi_display_status_reply(&mut p, Some(&NoWifiDisplay));
            }
            dm::GET_STABLE_DISPLAY_SIZE => {
                dm::GetStableDisplaySize::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match self.stable_display_size() {
                    Ok((w, h)) => {
                        dm::write_get_stable_display_size_reply(&mut p, Some(&Point(w, h)))
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                return reply(Err(self.host.unimplemented(
                    "display",
                    self.descriptor(),
                    code,
                )));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stable values are read as `loadFromXml` reads them, and set in
    /// a document as `saveToXml` writes them.
    /// The default `WifiDisplayStatus`, as `writeToParcel` writes it.
    #[test]
    fn no_wifi_display_is_the_default_status() {
        let mut p = Parcel::new();
        NoWifiDisplay.write_to(&mut p);
        let mut expected = Parcel::new();
        for v in [0, 0, 0, 0, 0, 1, 0] {
            expected.write_i32(v);
        }
        for _ in 0..3 {
            expected.write_string16(Some(""));
        }
        assert_eq!(p.data(), expected.data());
    }

    #[test]
    fn stable_values_are_read_and_set_as_the_store_does() {
        let xml = b"<display-manager-state><display-states /><stable-device-values><stable-display-width>1080</stable-display-width><stable-display-height>x</stable-display-height></stable-device-values></display-manager-state>";
        let root = aim_android_xml::read(xml).unwrap();
        assert_eq!(stable_values(&root), (1080, 0));
        assert_eq!(
            stable_values(&with_stable_values(Some(root), (1200, 1920))),
            (1200, 1920)
        );
        let fresh = with_stable_values(None, (800, 1280));
        assert_eq!(stable_values(&fresh), (800, 1280));
        assert_eq!(
            child_text(&fresh, "brightness-nits-for-default-display"),
            Some("-1.0")
        );
        assert_eq!(fresh.children().count(), 5);
    }

    /// The original's `getDisplayInfo(0)` reply to Calculator on the
    /// display server's 1080x1920 panel, ko-KR (captured on the pinned
    /// image).
    #[test]
    fn display_info_matches_the_original() {
        let display = Display::phone_for_test();
        let resources = Resources {
            name: "기본으로 제공되는 화면".into(),
            density_dpi: 320,
            brightness: (10, 255, 102),
            brightness_dim: 0.05,
        };
        let mut p = Parcel::new();
        dm::write_get_display_info_reply(
            &mut p,
            Some(&DisplayInfo {
                display: &display,
                resources: &resources,
            }),
        );
        assert_eq!(
            p.data(),
            include_bytes!("../../tests/fixtures/app_host/display-info.reply.bin")
        );
    }
}
