//! The Core Graphics and ImageIO calls icons are drawn with: an RGBA
//! bitmap context whose coordinates run from the top left, as Android's
//! drawables do, paths, gradients, images and PNG encoding.

use std::ffi::{CString, c_char, c_void};
use std::ptr::null;

type Ref = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }

    /// The rectangle `l`, `t`, `r`, `b` inside this one.
    pub fn inset(&self, l: f64, t: f64, r: f64, b: f64) -> Rect {
        Rect::new(self.x + l, self.y + t, self.w - l - r, self.h - t - b)
    }
}

/// `CGPoint` (and `CGSize`, which has the same layout).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: Ref;
    fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
    fn CGColorSpaceRelease(s: Ref);
    fn CGBitmapContextCreate(
        data: *mut c_void,
        w: usize,
        h: usize,
        bits: usize,
        row: usize,
        space: Ref,
        info: u32,
    ) -> Ref;
    fn CGBitmapContextCreateImage(c: Ref) -> Ref;
    fn CGContextRelease(c: Ref);
    fn CGImageRelease(i: Ref);
    fn CGImageGetWidth(i: Ref) -> usize;
    fn CGImageGetHeight(i: Ref) -> usize;
    fn CGContextSaveGState(c: Ref);
    fn CGContextRestoreGState(c: Ref);
    fn CGContextConcatCTM(c: Ref, t: Transform);
    fn CGContextSetRGBFillColor(c: Ref, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetRGBStrokeColor(c: Ref, r: f64, g: f64, b: f64, a: f64);
    fn CGContextSetLineWidth(c: Ref, w: f64);
    fn CGContextSetLineCap(c: Ref, cap: i32);
    fn CGContextSetLineJoin(c: Ref, join: i32);
    fn CGContextSetMiterLimit(c: Ref, limit: f64);
    fn CGContextSetAlpha(c: Ref, a: f64);
    fn CGContextSetInterpolationQuality(c: Ref, q: i32);
    fn CGContextSetBlendMode(c: Ref, mode: i32);
    fn CGContextBeginTransparencyLayer(c: Ref, aux: Ref);
    fn CGContextEndTransparencyLayer(c: Ref);
    fn CGContextAddPath(c: Ref, p: Ref);
    fn CGContextFillPath(c: Ref);
    fn CGContextEOFillPath(c: Ref);
    fn CGContextStrokePath(c: Ref);
    fn CGContextClip(c: Ref);
    fn CGContextEOClip(c: Ref);
    fn CGContextFillRect(c: Ref, r: Rect);
    fn CGContextDrawImage(c: Ref, r: Rect, i: Ref);
    fn CGContextSetShadowWithColor(c: Ref, offset: Point, blur: f64, color: Ref);
    fn CGColorCreateSRGB(r: f64, g: f64, b: f64, a: f64) -> Ref;
    fn CGColorRelease(c: Ref);
    fn CGPathCreateMutable() -> Ref;
    fn CGPathRelease(p: Ref);
    fn CGPathMoveToPoint(p: Ref, t: *const Transform, x: f64, y: f64);
    fn CGPathAddLineToPoint(p: Ref, t: *const Transform, x: f64, y: f64);
    fn CGPathAddCurveToPoint(
        p: Ref,
        t: *const Transform,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x: f64,
        y: f64,
    );
    fn CGPathAddQuadCurveToPoint(p: Ref, t: *const Transform, cx: f64, cy: f64, x: f64, y: f64);
    fn CGPathCloseSubpath(p: Ref);
    fn CGPathAddEllipseInRect(p: Ref, t: *const Transform, r: Rect);
    fn CGGradientCreateWithColorComponents(
        space: Ref,
        components: *const f64,
        locations: *const f64,
        count: usize,
    ) -> Ref;
    fn CGGradientRelease(g: Ref);
    fn CGContextDrawLinearGradient(c: Ref, g: Ref, start: Point, end: Point, options: u32);
    fn CGContextDrawRadialGradient(
        c: Ref,
        g: Ref,
        start: Point,
        r0: f64,
        end: Point,
        r1: f64,
        options: u32,
    );
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithData(data: Ref, options: Ref) -> Ref;
    fn CGImageSourceCreateImageAtIndex(s: Ref, i: usize, options: Ref) -> Ref;
    fn CGImageDestinationCreateWithData(data: Ref, uti: Ref, count: usize, options: Ref) -> Ref;
    fn CGImageDestinationAddImage(d: Ref, i: Ref, props: Ref);
    fn CGImageDestinationFinalize(d: Ref) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(alloc: Ref, bytes: *const u8, len: isize) -> Ref;
    fn CFDataCreateMutable(alloc: Ref, capacity: isize) -> Ref;
    fn CFDataGetBytePtr(d: Ref) -> *const u8;
    fn CFDataGetLength(d: Ref) -> isize;
    fn CFStringCreateWithCString(alloc: Ref, s: *const c_char, encoding: u32) -> Ref;
    fn CFRelease(r: Ref);
}

const PREMULTIPLIED_LAST: u32 = 1;
const UTF8: u32 = 0x0800_0100;
const INTERPOLATION_HIGH: i32 = 3;
const BLEND_SOURCE_IN: i32 = 18;
const GRADIENT_EXTEND: u32 = 3;

fn point([x, y]: [f64; 2]) -> Point {
    Point { x, y }
}

/// An ARGB color as components.
pub fn rgba(argb: u32) -> [f64; 4] {
    let c = |shift: u32| ((argb >> shift) & 0xff) as f64 / 255.0;
    [c(16), c(8), c(0), c(24)]
}

/// A decoded image (PNG, WebP, ...).
pub struct Image(Ref);

impl Image {
    pub fn decode(bytes: &[u8]) -> Option<Image> {
        // SAFETY: CF objects created and released here; the image is owned.
        unsafe {
            let data = CFDataCreate(null(), bytes.as_ptr(), bytes.len() as isize);
            let src = CGImageSourceCreateWithData(data, null());
            CFRelease(data);
            if src.is_null() {
                return None;
            }
            let img = CGImageSourceCreateImageAtIndex(src, 0, null());
            CFRelease(src);
            (!img.is_null()).then_some(Image(img))
        }
    }

    pub fn size(&self) -> (usize, usize) {
        // SAFETY: a live image.
        unsafe { (CGImageGetWidth(self.0), CGImageGetHeight(self.0)) }
    }

    /// PNG bytes.
    pub fn png(&self) -> Option<Vec<u8>> {
        let uti = CString::new("public.png").unwrap();
        // SAFETY: CF objects created and released here.
        unsafe {
            let data = CFDataCreateMutable(null(), 0);
            let ty = CFStringCreateWithCString(null(), uti.as_ptr(), UTF8);
            let dest = CGImageDestinationCreateWithData(data, ty, 1, null());
            CFRelease(ty);
            let ok = !dest.is_null() && {
                CGImageDestinationAddImage(dest, self.0, null());
                CGImageDestinationFinalize(dest)
            };
            if !dest.is_null() {
                CFRelease(dest);
            }
            let out = ok.then(|| {
                std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize)
                    .to_vec()
            });
            CFRelease(data);
            out
        }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: owned.
        unsafe { CGImageRelease(self.0) }
    }
}

/// A path in the coordinates it is drawn in.
pub struct Path(Ref);

impl Path {
    pub fn new() -> Path {
        // SAFETY: a new mutable path, owned.
        Path(unsafe { CGPathCreateMutable() })
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        // SAFETY: a live mutable path.
        unsafe { CGPathMoveToPoint(self.0, null(), x, y) }
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        // SAFETY: as above.
        unsafe { CGPathAddLineToPoint(self.0, null(), x, y) }
    }

    pub fn cubic_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        // SAFETY: as above.
        unsafe { CGPathAddCurveToPoint(self.0, null(), x1, y1, x2, y2, x, y) }
    }

    pub fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        // SAFETY: as above.
        unsafe { CGPathAddQuadCurveToPoint(self.0, null(), cx, cy, x, y) }
    }

    pub fn close(&mut self) {
        // SAFETY: as above.
        unsafe { CGPathCloseSubpath(self.0) }
    }

    pub fn ellipse(&mut self, r: Rect) {
        // SAFETY: as above.
        unsafe { CGPathAddEllipseInRect(self.0, null(), r) }
    }
}

impl Default for Path {
    fn default() -> Self {
        Path::new()
    }
}

impl Drop for Path {
    fn drop(&mut self) {
        // SAFETY: owned.
        unsafe { CGPathRelease(self.0) }
    }
}

/// How a path's stroke ends and joins (Android's `strokeLineCap`,
/// `strokeLineJoin`; Core Graphics numbers them the same way).
#[derive(Clone, Copy, Debug)]
pub struct Stroke {
    pub width: f64,
    pub cap: i32,
    pub join: i32,
    pub miter: f64,
}

/// A gradient: its stops (offset, ARGB) and its geometry.
#[derive(Clone, Debug)]
pub enum Gradient {
    Linear {
        stops: Vec<(f64, u32)>,
        from: [f64; 2],
        to: [f64; 2],
    },
    Radial {
        stops: Vec<(f64, u32)>,
        center: [f64; 2],
        radius: f64,
    },
}

/// A square RGBA bitmap drawn with the origin at the top left.
pub struct Canvas {
    ctx: Ref,
    pub size: usize,
}

impl Canvas {
    pub fn new(size: usize) -> Canvas {
        // SAFETY: a new bitmap context owning its memory; the flip makes y
        // run down.
        unsafe {
            let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
            let ctx = CGBitmapContextCreate(
                std::ptr::null_mut(),
                size,
                size,
                8,
                0,
                space,
                PREMULTIPLIED_LAST,
            );
            CGColorSpaceRelease(space);
            CGContextSetInterpolationQuality(ctx, INTERPOLATION_HIGH);
            CGContextConcatCTM(
                ctx,
                Transform {
                    a: 1.0,
                    b: 0.0,
                    c: 0.0,
                    d: -1.0,
                    tx: 0.0,
                    ty: size as f64,
                },
            );
            Canvas { ctx, size }
        }
    }

    pub fn image(&self) -> Image {
        // SAFETY: a live context; the image is owned.
        Image(unsafe { CGBitmapContextCreateImage(self.ctx) })
    }

    /// Run `f` with the drawing state saved around it.
    pub fn saved(&self, f: impl FnOnce()) {
        // SAFETY: balanced save and restore on a live context.
        unsafe { CGContextSaveGState(self.ctx) };
        f();
        unsafe { CGContextRestoreGState(self.ctx) };
    }

    pub fn transform(&self, t: Transform) {
        // SAFETY: a live context.
        unsafe { CGContextConcatCTM(self.ctx, t) }
    }

    pub fn alpha(&self, a: f64) {
        // SAFETY: a live context.
        unsafe { CGContextSetAlpha(self.ctx, a) }
    }

    pub fn clip(&self, p: &Path, even_odd: bool) {
        // SAFETY: a live context and path.
        unsafe {
            CGContextAddPath(self.ctx, p.0);
            if even_odd {
                CGContextEOClip(self.ctx)
            } else {
                CGContextClip(self.ctx)
            }
        }
    }

    pub fn fill(&self, p: &Path, argb: u32, even_odd: bool) {
        let [r, g, b, a] = rgba(argb);
        // SAFETY: a live context and path.
        unsafe {
            CGContextSetRGBFillColor(self.ctx, r, g, b, a);
            CGContextAddPath(self.ctx, p.0);
            if even_odd {
                CGContextEOFillPath(self.ctx)
            } else {
                CGContextFillPath(self.ctx)
            }
        }
    }

    pub fn stroke(&self, p: &Path, argb: u32, s: Stroke) {
        let [r, g, b, a] = rgba(argb);
        // SAFETY: a live context and path.
        unsafe {
            CGContextSetRGBStrokeColor(self.ctx, r, g, b, a);
            CGContextSetLineWidth(self.ctx, s.width);
            CGContextSetLineCap(self.ctx, s.cap);
            CGContextSetLineJoin(self.ctx, s.join);
            CGContextSetMiterLimit(self.ctx, s.miter);
            CGContextAddPath(self.ctx, p.0);
            CGContextStrokePath(self.ctx);
        }
    }

    /// Fill `p` with gradient `g`.
    pub fn fill_gradient(&self, p: &Path, g: &Gradient, even_odd: bool) {
        self.saved(|| {
            self.clip(p, even_odd);
            let stops = match g {
                Gradient::Linear { stops, .. } | Gradient::Radial { stops, .. } => stops,
            };
            let comps: Vec<f64> = stops.iter().flat_map(|&(_, c)| rgba(c)).collect();
            let locs: Vec<f64> = stops.iter().map(|&(o, _)| o).collect();
            // SAFETY: arrays of the stated counts; the gradient is released.
            unsafe {
                let space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
                let gr = CGGradientCreateWithColorComponents(
                    space,
                    comps.as_ptr(),
                    locs.as_ptr(),
                    locs.len(),
                );
                CGColorSpaceRelease(space);
                match g {
                    Gradient::Linear { from, to, .. } => CGContextDrawLinearGradient(
                        self.ctx,
                        gr,
                        point(*from),
                        point(*to),
                        GRADIENT_EXTEND,
                    ),
                    Gradient::Radial { center, radius, .. } => CGContextDrawRadialGradient(
                        self.ctx,
                        gr,
                        point(*center),
                        0.0,
                        point(*center),
                        *radius,
                        GRADIENT_EXTEND,
                    ),
                }
                CGGradientRelease(gr);
            }
        });
    }

    pub fn fill_rect(&self, r: Rect, argb: u32) {
        let [cr, g, b, a] = rgba(argb);
        // SAFETY: a live context.
        unsafe {
            CGContextSetRGBFillColor(self.ctx, cr, g, b, a);
            CGContextFillRect(self.ctx, r);
        }
    }

    /// Draw `img` into `r`, upright.
    pub fn draw_image(&self, img: &Image, r: Rect) {
        self.saved(|| {
            // Images draw bottom up: flip them back within `r`.
            self.transform(Transform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: -1.0,
                tx: 0.0,
                ty: 2.0 * r.y + r.h,
            });
            // SAFETY: a live context and image.
            unsafe { CGContextDrawImage(self.ctx, r, img.0) }
        });
    }

    /// Draw what `f` draws as one layer, then tint it: `argb` wherever it
    /// drew (Android's `SRC_IN` tint).
    pub fn tinted(&self, argb: u32, area: Rect, f: impl FnOnce()) {
        // SAFETY: a balanced transparency layer on a live context.
        unsafe { CGContextBeginTransparencyLayer(self.ctx, null()) };
        f();
        self.saved(|| {
            // SAFETY: a live context.
            unsafe { CGContextSetBlendMode(self.ctx, BLEND_SOURCE_IN) };
            self.fill_rect(area, argb);
        });
        unsafe { CGContextEndTransparencyLayer(self.ctx) };
    }

    /// Draw what `f` draws with a drop shadow under it.
    pub fn shadowed(&self, offset: [f64; 2], blur: f64, argb: u32, f: impl FnOnce()) {
        let [r, g, b, a] = rgba(argb);
        self.saved(|| {
            // SAFETY: a color created and released here; one layer so the
            // shadow is cast once.
            unsafe {
                let color = CGColorCreateSRGB(r, g, b, a);
                CGContextSetShadowWithColor(self.ctx, point(offset), blur, color);
                CGColorRelease(color);
                CGContextBeginTransparencyLayer(self.ctx, null());
            }
            f();
            unsafe { CGContextEndTransparencyLayer(self.ctx) };
        });
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: owned.
        unsafe { CGContextRelease(self.ctx) }
    }
}
