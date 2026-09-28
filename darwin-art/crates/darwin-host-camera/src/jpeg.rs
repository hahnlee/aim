//! JPEG encoding with ImageIO: a BGRA frame as a `CGImage`, written by a
//! `CGImageDestination` with the requested quality and the EXIF
//! orientation of `android.jpeg.orientation`.

use std::ffi::c_void;

use crate::avf::resolve;
use crate::convert::Bgra;
use crate::objc::{self, Framework, Id, Pool};

type CfRef = *mut c_void;

/// `kCGBitmapByteOrder32Little | kCGImageAlphaNoneSkipFirst`: B G R X in
/// memory.
const BITMAP_BGRX: u32 = (2 << 12) | 6;

pub struct Api {
    create_color_space: unsafe extern "C" fn() -> CfRef,
    create_provider: unsafe extern "C" fn(*mut c_void, *const u8, usize, *const c_void) -> CfRef,
    #[allow(clippy::type_complexity)]
    create_image: unsafe extern "C" fn(
        usize,
        usize,
        usize,
        usize,
        usize,
        CfRef,
        u32,
        CfRef,
        *const f64,
        bool,
        i32,
    ) -> CfRef,
    create_data: unsafe extern "C" fn(*const c_void, isize) -> CfRef,
    data_length: unsafe extern "C" fn(CfRef) -> isize,
    data_bytes: unsafe extern "C" fn(CfRef) -> *const u8,
    release: unsafe extern "C" fn(CfRef),
    create_destination: unsafe extern "C" fn(CfRef, Id, usize, Id) -> CfRef,
    add_image: unsafe extern "C" fn(CfRef, CfRef, Id),
    finalize: unsafe extern "C" fn(CfRef) -> bool,
    quality_key: Id,
    orientation_key: Id,
}

impl Api {
    pub fn load() -> Option<Api> {
        let cg =
            Framework::open(c"/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")?;
        let cf =
            Framework::open(c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")?;
        let io = Framework::open(c"/System/Library/Frameworks/ImageIO.framework/ImageIO")?;
        let api = Api {
            create_color_space: resolve!(cg, "CGColorSpaceCreateDeviceRGB"),
            create_provider: resolve!(cg, "CGDataProviderCreateWithData"),
            create_image: resolve!(cg, "CGImageCreate"),
            create_data: resolve!(cf, "CFDataCreateMutable"),
            data_length: resolve!(cf, "CFDataGetLength"),
            data_bytes: resolve!(cf, "CFDataGetBytePtr"),
            release: resolve!(cf, "CFRelease"),
            create_destination: resolve!(io, "CGImageDestinationCreateWithData"),
            add_image: resolve!(io, "CGImageDestinationAddImage"),
            finalize: resolve!(io, "CGImageDestinationFinalize"),
            quality_key: io.constant("kCGImageDestinationLossyCompressionQuality"),
            orientation_key: io.constant("kCGImagePropertyOrientation"),
        };
        Some(api)
    }
}

/// The EXIF orientation of a clockwise rotation in degrees.
pub fn exif_orientation(degrees: u32) -> u32 {
    match degrees % 360 {
        90 => 6,
        180 => 3,
        270 => 8,
        _ => 1,
    }
}

/// `frame` as a JPEG, or `None` when ImageIO fails.
pub fn encode(api: &Api, frame: &Bgra, quality: u32, rotation: u32) -> Option<Vec<u8>> {
    let _pool = Pool::new();
    // SAFETY: CoreGraphics and ImageIO objects created and released here;
    // the provider reads `frame`, which outlives the image and the
    // destination (both released before returning).
    unsafe {
        let space = (api.create_color_space)();
        let provider = (api.create_provider)(
            std::ptr::null_mut(),
            frame.data.as_ptr(),
            frame.data.len(),
            std::ptr::null(),
        );
        let image = (api.create_image)(
            frame.width as usize,
            frame.height as usize,
            8,
            32,
            frame.stride,
            space,
            BITMAP_BGRX,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        let data = (api.create_data)(std::ptr::null(), 0);
        let dest = (api.create_destination)(data, objc::nsstring("public.jpeg"), 1, objc::NIL);
        let mut out = None;
        if !image.is_null() && !dest.is_null() {
            let props = objc::dictionary(&[
                (
                    api.quality_key,
                    objc::number_f64(quality.clamp(1, 100) as f64 / 100.0),
                ),
                (
                    api.orientation_key,
                    objc::number_u32(exif_orientation(rotation)),
                ),
            ]);
            (api.add_image)(dest, image, props);
            if (api.finalize)(dest) {
                let len = (api.data_length)(data).max(0) as usize;
                out = Some(std::slice::from_raw_parts((api.data_bytes)(data), len).to_vec());
            }
        }
        for r in [dest, data, image, provider, space] {
            if !r.is_null() {
                (api.release)(r);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_decodable_jpeg() {
        let Some(api) = crate::avf::api() else { return };
        let mut f = Bgra::new(64, 48);
        crate::pattern::draw(&mut f, 0);
        let jpeg = encode(&api.jpeg, &f, 90, 90).expect("encode");
        assert_eq!(&jpeg[..2], &[0xff, 0xd8]);
        assert_eq!(&jpeg[jpeg.len() - 2..], &[0xff, 0xd9]);
        // The EXIF orientation tag (0x0112) is present with value 6.
        let tag = jpeg
            .windows(2)
            .position(|w| w == [0x01, 0x12] || w == [0x12, 0x01]);
        assert!(tag.is_some(), "no orientation tag");
        assert_eq!(exif_orientation(270), 8);
        assert_eq!(exif_orientation(0), 1);
    }
}
