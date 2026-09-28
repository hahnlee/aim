//! What a Mac camera looks like to Android: the static metadata of a
//! LIMITED device with the `BACKWARD_COMPATIBLE` capability, its request
//! templates, and the dynamic part of each result.
//!
//! Output sizes are the camera's AVFoundation format sizes that fit in the
//! largest-width one, which is the active array. Every size streams
//! YUV_420_888 and IMPLEMENTATION_DEFINED at 30 fps and captures JPEG.
//! The camera runs its own exposure, white balance and focus, so the
//! controls are the AUTO modes alone and the 3A states are reported
//! converged. Optical properties AVFoundation does not report on macOS
//! (focal length, sensor size) are nominal values for a laptop camera's
//! 70° horizontal field of view.

use android_hardware_camera_metadata::aidl::android::hardware::camera::metadata::CameraMetadataTag::CameraMetadataTag as Tag;
use aim_hostcall::camera::{Size, facing};

use crate::metadata::Metadata;

/// `android.hardware.graphics.common.PixelFormat` values of the streams.
pub mod format {
    pub const RGBX_8888: i32 = 0x2;
    pub const BLOB: i32 = 0x21;
    pub const IMPLEMENTATION_DEFINED: i32 = 0x22;
    pub const YCBCR_420_888: i32 = 0x23;
}

pub const FPS: i32 = 30;
pub const FRAME_DURATION_NS: i64 = 1_000_000_000 / FPS as i64;
/// JPEG encoding of a full-size frame, the stall of a BLOB stream.
const JPEG_STALL_NS: i64 = 100_000_000;
pub const PIPELINE_DEPTH: u8 = 4;
/// `JPEG_MAX_SIZE`: generous for a 4:2:0 frame at quality 100, plus the
/// `CameraBlob` trailer.
pub fn jpeg_max_size(width: u32, height: u32) -> i32 {
    (width as i64 * height as i64 * 3 / 2 + 64 * 1024).min(i32::MAX as i64) as i32
}

/// Nominal optics: 4 mm focal length on a sensor sized for a 70° field.
const FOCAL_LENGTH_MM: f32 = 4.0;
const APERTURE: f32 = 2.0;

/// The static description of one camera.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub facing: u32,
    /// Output sizes, largest first; the first is the active array.
    pub sizes: Vec<(u32, u32)>,
}

impl Camera {
    /// The camera of AVFoundation's sizes, or `None` when it has none.
    pub fn new(facing: u32, sizes: &[Size]) -> Option<Camera> {
        let (aw, ah) = sizes
            .iter()
            .map(|s| (s.width, s.height))
            .max_by_key(|&(w, h)| (w, h))?;
        let mut out: Vec<(u32, u32)> = sizes
            .iter()
            .filter(|s| s.width <= aw && s.height <= ah && s.max_fps >= FPS as u32)
            .map(|s| (s.width, s.height))
            .collect();
        out.sort_by_key(|&(w, h)| std::cmp::Reverse(w as u64 * h as u64));
        out.dedup();
        (!out.is_empty()).then_some(Camera { facing, sizes: out })
    }

    pub fn active(&self) -> (u32, u32) {
        self.sizes[0]
    }

    pub fn supports(&self, format: i32, width: u32, height: u32) -> bool {
        matches!(
            format,
            format::YCBCR_420_888 | format::IMPLEMENTATION_DEFINED | format::BLOB
        ) && self.sizes.contains(&(width, height))
    }

    /// The static metadata (`getCameraCharacteristics`).
    pub fn characteristics(&self) -> Metadata {
        let (aw, ah) = self.active();
        let (w, h) = (aw as i32, ah as i32);
        // Sensor size for the nominal 70° horizontal field at the focal
        // length, with square pixels.
        let sensor_w = 2.0 * FOCAL_LENGTH_MM * (35f32.to_radians()).tan();
        let sensor_h = sensor_w * ah as f32 / aw as f32;
        let mut configs = Vec::new();
        let mut durations = Vec::new();
        let mut stalls = Vec::new();
        for &(sw, sh) in &self.sizes {
            for f in [
                format::IMPLEMENTATION_DEFINED,
                format::YCBCR_420_888,
                format::BLOB,
            ] {
                let (sw, sh) = (sw as i32, sh as i32);
                configs.extend([f, sw, sh, 0]);
                durations.extend([f as i64, sw as i64, sh as i64, FRAME_DURATION_NS]);
                let stall = if f == format::BLOB { JPEG_STALL_NS } else { 0 };
                stalls.extend([f as i64, sw as i64, sh as i64, stall]);
            }
        }
        let lens_facing = match self.facing {
            facing::FRONT => 0,
            facing::BACK => 1,
            _ => 2,
        };
        let mut m = Metadata::new();
        m.u8s(
            Tag::ANDROID_COLOR_CORRECTION_AVAILABLE_ABERRATION_MODES,
            &[0],
        )
        .u8s(Tag::ANDROID_CONTROL_AE_AVAILABLE_ANTIBANDING_MODES, &[3])
        .u8s(Tag::ANDROID_CONTROL_AE_AVAILABLE_MODES, &[1])
        .i32s(
            Tag::ANDROID_CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES,
            &[15, FPS, FPS, FPS],
        )
        .i32s(Tag::ANDROID_CONTROL_AE_COMPENSATION_RANGE, &[0, 0])
        .rationals(Tag::ANDROID_CONTROL_AE_COMPENSATION_STEP, &[(0, 1)])
        .u8s(Tag::ANDROID_CONTROL_AE_LOCK_AVAILABLE, &[0])
        .u8s(Tag::ANDROID_CONTROL_AF_AVAILABLE_MODES, &[0])
        .u8s(Tag::ANDROID_CONTROL_AVAILABLE_EFFECTS, &[0])
        .u8s(Tag::ANDROID_CONTROL_AVAILABLE_MODES, &[1])
        .u8s(Tag::ANDROID_CONTROL_AVAILABLE_SCENE_MODES, &[0])
        .u8s(
            Tag::ANDROID_CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES,
            &[0],
        )
        .u8s(Tag::ANDROID_CONTROL_AWB_AVAILABLE_MODES, &[1])
        .u8s(Tag::ANDROID_CONTROL_AWB_LOCK_AVAILABLE, &[0])
        .i32s(Tag::ANDROID_CONTROL_MAX_REGIONS, &[0, 0, 0])
        .u8s(Tag::ANDROID_EDGE_AVAILABLE_EDGE_MODES, &[0])
        .u8s(Tag::ANDROID_FLASH_INFO_AVAILABLE, &[0])
        .u8s(Tag::ANDROID_HOT_PIXEL_AVAILABLE_HOT_PIXEL_MODES, &[0])
        .i32s(Tag::ANDROID_JPEG_AVAILABLE_THUMBNAIL_SIZES, &[0, 0])
        .i32s(Tag::ANDROID_JPEG_MAX_SIZE, &[jpeg_max_size(aw, ah)])
        .u8s(Tag::ANDROID_LENS_FACING, &[lens_facing])
        .f32s(Tag::ANDROID_LENS_INFO_AVAILABLE_APERTURES, &[APERTURE])
        .f32s(Tag::ANDROID_LENS_INFO_AVAILABLE_FILTER_DENSITIES, &[0.0])
        .f32s(
            Tag::ANDROID_LENS_INFO_AVAILABLE_FOCAL_LENGTHS,
            &[FOCAL_LENGTH_MM],
        )
        .u8s(Tag::ANDROID_LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION, &[0])
        .u8s(Tag::ANDROID_LENS_INFO_FOCUS_DISTANCE_CALIBRATION, &[0])
        .f32s(Tag::ANDROID_LENS_INFO_HYPERFOCAL_DISTANCE, &[0.0])
        .f32s(Tag::ANDROID_LENS_INFO_MINIMUM_FOCUS_DISTANCE, &[0.0])
        .u8s(
            Tag::ANDROID_NOISE_REDUCTION_AVAILABLE_NOISE_REDUCTION_MODES,
            &[0],
        )
        .u8s(Tag::ANDROID_REQUEST_AVAILABLE_CAPABILITIES, &[0])
        .i32s(Tag::ANDROID_REQUEST_MAX_NUM_OUTPUT_STREAMS, &[0, 2, 1])
        .i32s(Tag::ANDROID_REQUEST_PARTIAL_RESULT_COUNT, &[1])
        .u8s(Tag::ANDROID_REQUEST_PIPELINE_MAX_DEPTH, &[PIPELINE_DEPTH])
        .f32s(Tag::ANDROID_SCALER_AVAILABLE_MAX_DIGITAL_ZOOM, &[1.0])
        .i64s(
            Tag::ANDROID_SCALER_AVAILABLE_MIN_FRAME_DURATIONS,
            &durations,
        )
        .i64s(Tag::ANDROID_SCALER_AVAILABLE_STALL_DURATIONS, &stalls)
        .i32s(
            Tag::ANDROID_SCALER_AVAILABLE_STREAM_CONFIGURATIONS,
            &configs,
        )
        .u8s(Tag::ANDROID_SCALER_CROPPING_TYPE, &[0])
        .i32s(Tag::ANDROID_SENSOR_AVAILABLE_TEST_PATTERN_MODES, &[0])
        .i32s(Tag::ANDROID_SENSOR_INFO_ACTIVE_ARRAY_SIZE, &[0, 0, w, h])
        .i64s(
            Tag::ANDROID_SENSOR_INFO_MAX_FRAME_DURATION,
            &[1_000_000_000 / 15],
        )
        .f32s(
            Tag::ANDROID_SENSOR_INFO_PHYSICAL_SIZE,
            &[sensor_w, sensor_h],
        )
        .i32s(Tag::ANDROID_SENSOR_INFO_PIXEL_ARRAY_SIZE, &[w, h])
        .i32s(
            Tag::ANDROID_SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE,
            &[0, 0, w, h],
        )
        .u8s(Tag::ANDROID_SENSOR_INFO_TIMESTAMP_SOURCE, &[1])
        .i32s(Tag::ANDROID_SENSOR_ORIENTATION, &[0])
        .u8s(Tag::ANDROID_SHADING_AVAILABLE_MODES, &[0])
        .u8s(
            Tag::ANDROID_STATISTICS_INFO_AVAILABLE_FACE_DETECT_MODES,
            &[0],
        )
        .u8s(
            Tag::ANDROID_STATISTICS_INFO_AVAILABLE_HOT_PIXEL_MAP_MODES,
            &[0],
        )
        .u8s(
            Tag::ANDROID_STATISTICS_INFO_AVAILABLE_LENS_SHADING_MAP_MODES,
            &[0],
        )
        .i32s(Tag::ANDROID_STATISTICS_INFO_MAX_FACE_COUNT, &[0])
        .i32s(Tag::ANDROID_SYNC_MAX_LATENCY, &[-1])
        .u8s(Tag::ANDROID_TONEMAP_AVAILABLE_TONE_MAP_MODES, &[1])
        .u8s(Tag::ANDROID_INFO_SUPPORTED_HARDWARE_LEVEL, &[0]);
        let request = template(self, 1);
        let request_keys: Vec<i32> = request.tags().map(|t| t as i32).collect();
        let mut result_keys = request_keys.clone();
        result_keys.extend(result_only_tags().iter().map(|t| t.0));
        result_keys.sort();
        m.i32s(Tag::ANDROID_REQUEST_AVAILABLE_REQUEST_KEYS, &request_keys)
            .i32s(Tag::ANDROID_REQUEST_AVAILABLE_RESULT_KEYS, &result_keys);
        let mut keys: Vec<i32> = m.tags().map(|t| t as i32).collect();
        keys.push(Tag::ANDROID_REQUEST_AVAILABLE_CHARACTERISTICS_KEYS.0);
        keys.sort();
        m.i32s(Tag::ANDROID_REQUEST_AVAILABLE_CHARACTERISTICS_KEYS, &keys);
        m
    }
}

/// `RequestTemplate`s this device offers.
pub fn template_supported(t: i32) -> bool {
    (1..=4).contains(&t)
}

/// The default request of template `t` (`constructDefaultRequestSettings`):
/// PREVIEW 1, STILL_CAPTURE 2, VIDEO_RECORD 3, VIDEO_SNAPSHOT 4. The
/// capture intent values equal the template numbers.
pub fn template(camera: &Camera, t: i32) -> Metadata {
    let (w, h) = camera.active();
    let fps = if t == 3 || t == 4 {
        [FPS, FPS]
    } else {
        [15, FPS]
    };
    let mut m = Metadata::new();
    m.u8s(Tag::ANDROID_COLOR_CORRECTION_ABERRATION_MODE, &[0])
        .u8s(Tag::ANDROID_CONTROL_AE_ANTIBANDING_MODE, &[3])
        .i32s(Tag::ANDROID_CONTROL_AE_EXPOSURE_COMPENSATION, &[0])
        .u8s(Tag::ANDROID_CONTROL_AE_LOCK, &[0])
        .u8s(Tag::ANDROID_CONTROL_AE_MODE, &[1])
        .u8s(Tag::ANDROID_CONTROL_AE_PRECAPTURE_TRIGGER, &[0])
        .i32s(Tag::ANDROID_CONTROL_AE_TARGET_FPS_RANGE, &fps)
        .u8s(Tag::ANDROID_CONTROL_AF_MODE, &[0])
        .u8s(Tag::ANDROID_CONTROL_AF_TRIGGER, &[0])
        .u8s(Tag::ANDROID_CONTROL_AWB_LOCK, &[0])
        .u8s(Tag::ANDROID_CONTROL_AWB_MODE, &[1])
        .u8s(Tag::ANDROID_CONTROL_CAPTURE_INTENT, &[t as u8])
        .u8s(Tag::ANDROID_CONTROL_EFFECT_MODE, &[0])
        .u8s(Tag::ANDROID_CONTROL_MODE, &[1])
        .u8s(Tag::ANDROID_CONTROL_SCENE_MODE, &[0])
        .u8s(Tag::ANDROID_CONTROL_VIDEO_STABILIZATION_MODE, &[0])
        .u8s(Tag::ANDROID_EDGE_MODE, &[0])
        .u8s(Tag::ANDROID_FLASH_MODE, &[0])
        .u8s(Tag::ANDROID_HOT_PIXEL_MODE, &[0])
        .i32s(Tag::ANDROID_JPEG_ORIENTATION, &[0])
        .u8s(Tag::ANDROID_JPEG_QUALITY, &[95])
        .u8s(Tag::ANDROID_JPEG_THUMBNAIL_QUALITY, &[85])
        .i32s(Tag::ANDROID_JPEG_THUMBNAIL_SIZE, &[0, 0])
        .f32s(Tag::ANDROID_LENS_APERTURE, &[APERTURE])
        .f32s(Tag::ANDROID_LENS_FILTER_DENSITY, &[0.0])
        .f32s(Tag::ANDROID_LENS_FOCAL_LENGTH, &[FOCAL_LENGTH_MM])
        .f32s(Tag::ANDROID_LENS_FOCUS_DISTANCE, &[0.0])
        .u8s(Tag::ANDROID_LENS_OPTICAL_STABILIZATION_MODE, &[0])
        .u8s(Tag::ANDROID_NOISE_REDUCTION_MODE, &[0])
        .i32s(Tag::ANDROID_SCALER_CROP_REGION, &[0, 0, w as i32, h as i32])
        .i32s(Tag::ANDROID_SENSOR_TEST_PATTERN_MODE, &[0])
        .u8s(Tag::ANDROID_SHADING_MODE, &[0])
        .u8s(Tag::ANDROID_STATISTICS_FACE_DETECT_MODE, &[0])
        .u8s(Tag::ANDROID_STATISTICS_HOT_PIXEL_MAP_MODE, &[0])
        .u8s(Tag::ANDROID_STATISTICS_LENS_SHADING_MAP_MODE, &[0])
        .u8s(Tag::ANDROID_TONEMAP_MODE, &[1]);
    m
}

/// Result keys that are not request keys.
fn result_only_tags() -> [Tag; 10] {
    [
        Tag::ANDROID_CONTROL_AE_STATE,
        Tag::ANDROID_CONTROL_AF_STATE,
        Tag::ANDROID_CONTROL_AWB_STATE,
        Tag::ANDROID_FLASH_STATE,
        Tag::ANDROID_LENS_STATE,
        Tag::ANDROID_REQUEST_PIPELINE_DEPTH,
        Tag::ANDROID_SENSOR_FRAME_DURATION,
        Tag::ANDROID_SENSOR_ROLLING_SHUTTER_SKEW,
        Tag::ANDROID_SENSOR_TIMESTAMP,
        Tag::ANDROID_STATISTICS_SCENE_FLICKER,
    ]
}

/// The result of a capture: the request's settings, echoed, and the
/// frame's dynamic state.
pub fn result(settings: &Metadata, timestamp_ns: i64) -> Metadata {
    let mut m = settings.clone();
    // AE/AWB converged (2), AF inactive (0), no flash (0 unavailable),
    // lens stationary (0), no flicker detected (0).
    m.u8s(Tag::ANDROID_CONTROL_AE_STATE, &[2])
        .u8s(Tag::ANDROID_CONTROL_AF_STATE, &[0])
        .u8s(Tag::ANDROID_CONTROL_AWB_STATE, &[2])
        .u8s(Tag::ANDROID_FLASH_STATE, &[0])
        .u8s(Tag::ANDROID_LENS_STATE, &[0])
        .u8s(Tag::ANDROID_REQUEST_PIPELINE_DEPTH, &[PIPELINE_DEPTH])
        .i64s(Tag::ANDROID_SENSOR_FRAME_DURATION, &[FRAME_DURATION_NS])
        .i64s(
            Tag::ANDROID_SENSOR_ROLLING_SHUTTER_SKEW,
            &[FRAME_DURATION_NS / 2],
        )
        .i64s(Tag::ANDROID_SENSOR_TIMESTAMP, &[timestamp_ns])
        .u8s(Tag::ANDROID_STATISTICS_SCENE_FLICKER, &[0]);
    m
}

/// The format a stream's buffers are allocated in: IMPLEMENTATION_DEFINED
/// becomes RGBX when the consumer samples or composes it (the host GPU
/// has no YUV textures, docs/graphics-buffers.md), YUV_420_888 otherwise
/// (video encoders, CPU readers).
pub fn override_format(format: i32, consumer_usage: i64) -> i32 {
    // GPU_TEXTURE, GPU_RENDER_TARGET, COMPOSER_OVERLAY,
    // COMPOSER_CLIENT_TARGET, COMPOSER_CURSOR.
    const GPU: i64 = (1 << 8) | (1 << 9) | (1 << 11) | (1 << 12) | (1 << 15);
    match format {
        format::IMPLEMENTATION_DEFINED if consumer_usage & GPU != 0 => format::RGBX_8888,
        format::IMPLEMENTATION_DEFINED => format::YCBCR_420_888,
        f => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facetime() -> Camera {
        let s = |width, height| Size {
            width,
            height,
            max_fps: 30,
            reserved: 0,
        };
        // The sizes of a MacBook Pro's FaceTime HD camera, from
        // AVFoundation, largest area first.
        Camera::new(
            facing::FRONT,
            &[
                s(1552, 1552),
                s(1760, 1328),
                s(1920, 1080),
                s(1280, 720),
                s(640, 480),
            ],
        )
        .unwrap()
    }

    #[test]
    fn sizes_fit_in_the_widest() {
        let c = facetime();
        assert_eq!(c.active(), (1920, 1080));
        assert_eq!(c.sizes, [(1920, 1080), (1280, 720), (640, 480)]);
        assert!(c.supports(format::BLOB, 640, 480));
        assert!(!c.supports(0x1, 640, 480));
        assert!(!c.supports(format::YCBCR_420_888, 1552, 1552));
        assert!(Camera::new(facing::FRONT, &[]).is_none());
    }

    #[test]
    fn characteristics_of_a_limited_front_camera() {
        let m = facetime().characteristics();
        assert_eq!(m.u8(Tag::ANDROID_INFO_SUPPORTED_HARDWARE_LEVEL), Some(0));
        assert_eq!(m.u8(Tag::ANDROID_LENS_FACING), Some(0));
        assert_eq!(m.u8(Tag::ANDROID_REQUEST_AVAILABLE_CAPABILITIES), Some(0));
        assert_eq!(m.u8(Tag::ANDROID_SENSOR_INFO_TIMESTAMP_SOURCE), Some(1));
        let configs = m
            .get(Tag::ANDROID_SCALER_AVAILABLE_STREAM_CONFIGURATIONS)
            .unwrap()
            .i32s();
        assert_eq!(configs.len(), 3 * 3 * 4);
        assert_eq!(
            &configs[..4],
            &[format::IMPLEMENTATION_DEFINED, 1920, 1080, 0]
        );
        assert!(configs.chunks(4).any(|c| c == [format::BLOB, 640, 480, 0]));
        assert_eq!(
            m.get(Tag::ANDROID_SENSOR_INFO_ACTIVE_ARRAY_SIZE)
                .unwrap()
                .i32s(),
            [0, 0, 1920, 1080]
        );
        // Every key is listed, and the lists survive a round trip.
        let keys = m
            .get(Tag::ANDROID_REQUEST_AVAILABLE_CHARACTERISTICS_KEYS)
            .unwrap()
            .i32s();
        assert_eq!(keys.len(), m.tags().count());
        assert!(keys.iter().zip(m.tags()).all(|(&k, t)| k as u32 == t));
        assert_eq!(Metadata::parse(&m.to_bytes()), Some(m));
    }

    #[test]
    fn templates_and_results() {
        let c = facetime();
        assert!(template_supported(1) && template_supported(4));
        assert!(!template_supported(5) && !template_supported(6));
        let video = template(&c, 3);
        assert_eq!(video.u8(Tag::ANDROID_CONTROL_CAPTURE_INTENT), Some(3));
        assert_eq!(
            video
                .get(Tag::ANDROID_CONTROL_AE_TARGET_FPS_RANGE)
                .unwrap()
                .i32s(),
            [30, 30]
        );
        let r = result(&template(&c, 1), 12345);
        let ts = r.get(Tag::ANDROID_SENSOR_TIMESTAMP).unwrap();
        assert_eq!(ts.bytes, 12345i64.to_le_bytes());
        assert_eq!(r.u8(Tag::ANDROID_CONTROL_AE_STATE), Some(2));
        assert_eq!(r.u8(Tag::ANDROID_CONTROL_MODE), Some(1));
        // Every result tag is an advertised result key.
        let keys = c
            .characteristics()
            .get(Tag::ANDROID_REQUEST_AVAILABLE_RESULT_KEYS)
            .unwrap()
            .i32s();
        assert!(r.tags().all(|t| keys.contains(&(t as i32))));
    }

    #[test]
    fn preview_buffers_are_rgbx_for_the_gpu() {
        let texture = 1 << 8;
        let encoder = 1 << 16;
        assert_eq!(
            override_format(format::IMPLEMENTATION_DEFINED, texture),
            format::RGBX_8888
        );
        assert_eq!(
            override_format(format::IMPLEMENTATION_DEFINED, encoder),
            format::YCBCR_420_888
        );
        assert_eq!(override_format(format::BLOB, texture), format::BLOB);
    }
}
