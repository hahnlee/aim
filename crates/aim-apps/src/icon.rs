//! An app's launcher icon as a macOS app icon.
//!
//! macOS draws app icons on a grid of 1024 pixels: the body is a rounded
//! square of 824 with continuous corners (the "squircle"), 100 in from each
//! edge, over a soft drop shadow (the Human Interface Guidelines' macOS
//! icon template).
//!
//! - **Adaptive icons** (`<adaptive-icon>`): the background and foreground
//!   layers are drawn on Android's 108 dp canvas, whose 72 dp viewport
//!   fills the body, and the body's shape clips them, as a launcher's mask
//!   would. So the foreground's 66 dp safe zone is never cut.
//! - **Legacy icons** (a bitmap or a vector): drawn at three quarters of
//!   the body on a white plate of the body's shape, never stretched.
//!
//! The `.icns` holds every size macOS asks for, 16 to 1024 pixels at 1x
//! and 2x, as PNG.

use crate::apk::Resources;
use crate::cg::{Canvas, Image, Path, Rect};
use crate::drawable::Drawer;
use crate::res::{Element, Value};

/// The macOS icon grid, at 1024 pixels.
const GRID: f64 = 1024.0;
const BODY: f64 = 824.0;
const CORNER: f64 = 185.4;
/// The drop shadow: down 12 and blurred 28 at 1024, black at 30 %.
const SHADOW: ([f64; 2], f64, u32) = ([0.0, -12.0], 28.0, 0x4d00_0000);
/// Android's adaptive icon canvas and the part a mask shows, in dp.
const CANVAS_DP: f64 = 108.0;
const VIEWPORT_DP: f64 = 72.0;
/// A legacy icon's size on the plate, of the body.
const LEGACY: f64 = 0.75;
const PLATE: u32 = 0xffff_ffff;

/// An adaptive icon's layer: a drawable resource, or one written inline.
#[derive(Clone, Debug)]
pub enum Layer {
    Resource(Value),
    Inline(Element),
}

/// A launcher icon's layers.
#[derive(Clone, Debug)]
pub enum Icon {
    Adaptive {
        background: Option<Layer>,
        foreground: Option<Layer>,
        /// The single-color layer, for tinted and template icons.
        monochrome: Option<Layer>,
    },
    Legacy(Value),
}

impl Icon {
    /// The icon resource `v` (`android:icon`) as layers.
    pub fn of(res: &Resources, v: &Value) -> Option<Icon> {
        let (resolved, apk) = res.resolve(v)?;
        if let Value::String(path) = &resolved
            && path.ends_with(".xml")
            && let Ok(e) = crate::res::xml(&apk.file(path).ok()?)
            && e.name == "adaptive-icon"
        {
            let layer = |name: &str| {
                let part = e.children.iter().find(|c| c.name == name)?;
                match part.named("drawable") {
                    Some(v) => Some(Layer::Resource(v.clone())),
                    None => part.children.first().cloned().map(Layer::Inline),
                }
            };
            return Some(Icon::Adaptive {
                background: layer("background"),
                foreground: layer("foreground"),
                monochrome: layer("monochrome"),
            });
        }
        Some(Icon::Legacy(v.clone()))
    }
}

/// A rounded rectangle with continuous corners, as macOS and iOS draw
/// them: each corner's curve starts 1.528 radii from the corner.
fn squircle(r: Rect, radius: f64) -> Path {
    let rad = radius.min(r.w.min(r.h) / 2.0 / 1.528_664_83);
    let (x, y, w, h) = (r.x, r.y, r.w, r.h);
    let mut p = Path::new();
    // Offsets along an edge and into the corner, as multiples of `rad`.
    const A: f64 = 1.528_664_83;
    const B: f64 = 1.088_492_96;
    const C: f64 = 0.868_406_94;
    const D: f64 = 0.631_493_99;
    const E: f64 = 0.074_911_00;
    const F: f64 = 0.372_823_92;
    const G: f64 = 0.169_055_77;
    let corner = |p: &mut Path, cx: f64, cy: f64, sx: f64, sy: f64, flip: bool| {
        // A corner from the edge along x to the edge along y (or back when
        // `flip`), in the quadrant `sx`, `sy` of corner (cx, cy).
        let pt = |u: f64, v: f64| {
            if flip {
                (cx + sx * v * rad, cy + sy * u * rad)
            } else {
                (cx + sx * u * rad, cy + sy * v * rad)
            }
        };
        let (a, b) = (pt(B, 0.0), pt(C, 0.0));
        let e = pt(D, E);
        p.cubic_to(a.0, a.1, b.0, b.1, e.0, e.1);
        let (c1, c2, e2) = (pt(F, G), pt(G, F), pt(E, D));
        p.cubic_to(c1.0, c1.1, c2.0, c2.1, e2.0, e2.1);
        let (c3, c4, e3) = (pt(0.0, C), pt(0.0, B), pt(0.0, A));
        p.cubic_to(c3.0, c3.1, c4.0, c4.1, e3.0, e3.1);
    };
    p.move_to(x + A * rad, y);
    p.line_to(x + w - A * rad, y);
    corner(&mut p, x + w, y, -1.0, 1.0, false);
    p.line_to(x + w, y + h - A * rad);
    corner(&mut p, x + w, y + h, -1.0, -1.0, true);
    p.line_to(x + A * rad, y + h);
    corner(&mut p, x, y + h, 1.0, -1.0, false);
    p.line_to(x, y + A * rad);
    corner(&mut p, x, y, 1.0, 1.0, true);
    p.close();
    p
}

/// The icon drawn `size` pixels square on the macOS grid.
pub fn render(res: &Resources, icon: &Icon, size: usize) -> Image {
    let canvas = Canvas::new(size);
    let s = size as f64 / GRID;
    let margin = (GRID - BODY) / 2.0 * s;
    let body = Rect::new(margin, margin, BODY * s, BODY * s);
    let shape = squircle(body, CORNER * s);
    let (offset, blur, shadow) = SHADOW;
    canvas.shadowed([offset[0] * s, offset[1] * s], blur * s, shadow, || {
        canvas.saved(|| {
            canvas.clip(&shape, false);
            match icon {
                Icon::Adaptive {
                    background,
                    foreground,
                    ..
                } => {
                    let dp = body.w / VIEWPORT_DP;
                    let extent = CANVAS_DP * dp;
                    let c = body.x + body.w / 2.0;
                    let layer = Rect::new(c - extent / 2.0, c - extent / 2.0, extent, extent);
                    let d = Drawer {
                        res,
                        canvas: &canvas,
                        dp,
                    };
                    // An adaptive icon's background is opaque: white where
                    // it draws nothing.
                    canvas.fill_rect(body, PLATE);
                    for l in [background, foreground].into_iter().flatten() {
                        match l {
                            Layer::Resource(v) => d.draw(v, layer),
                            Layer::Inline(e) => d.draw_element(e, layer),
                        };
                    }
                }
                Icon::Legacy(v) => {
                    canvas.fill_rect(body, PLATE);
                    let side = body.w * LEGACY;
                    let c = body.x + body.w / 2.0;
                    let r = Rect::new(c - side / 2.0, c - side / 2.0, side, side);
                    // A legacy icon is 48 dp.
                    let d = Drawer {
                        res,
                        canvas: &canvas,
                        dp: side / 48.0,
                    };
                    d.draw(v, r);
                }
            }
        });
    });
    canvas.image()
}

/// `.icns` entry types and their sizes in pixels: 16, 32, 128, 256 and
/// 512 points at 1x and 2x, and 64 (32@2x's 1x twin).
const ICNS: [(&[u8; 4], usize); 11] = [
    (b"icp4", 16),
    (b"icp5", 32),
    (b"ic11", 32),
    (b"icp6", 64),
    (b"ic12", 64),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic13", 256),
    (b"ic09", 512),
    (b"ic14", 512),
    (b"ic10", 1024),
];

/// The icon as an `.icns`, every size drawn at its own resolution.
pub fn icns(res: &Resources, icon: &Icon) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut pngs: Vec<(usize, Vec<u8>)> = Vec::new();
    for (kind, size) in ICNS {
        let png = match pngs.iter().find(|(s, _)| *s == size) {
            Some((_, png)) => png.clone(),
            None => {
                let png = render(res, icon, size).png()?;
                pngs.push((size, png.clone()));
                png
            }
        };
        body.extend_from_slice(kind);
        body.extend_from_slice(&(8 + png.len() as u32).to_be_bytes());
        body.extend_from_slice(&png);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(8 + body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Some(out)
}
