//! Drawing an app's drawables the way Android draws them, for its icon:
//! colors, bitmaps (PNG, WebP), vector drawables with their groups, clip
//! paths, strokes and gradients, and the containers launcher icons use
//! (`inset`, `layer-list`, `bitmap`, `shape`, `selector`, ...).
//!
//! Lengths in dp are drawn at the canvas's scale for the drawable's frame;
//! theme attributes resolve against the app's theme.

use crate::apk::{Apk, Resources};
use crate::cg::{Canvas, Gradient, Image, Path, Rect, Stroke, Transform};
use crate::res::{Element, Value};

/// How deep drawables may nest (a drawable referring to itself ends).
const MAX_DEPTH: u32 = 16;

pub struct Drawer<'a> {
    pub res: &'a Resources<'a>,
    pub canvas: &'a Canvas,
    /// Canvas pixels per dp.
    pub dp: f64,
}

enum Paint {
    Solid(u32),
    Gradient(Gradient),
}

/// A float attribute.
fn num(e: &Element, name: &str, default: f64) -> f64 {
    match e.named(name) {
        Some(v) => v.length(1.0).map_or(default, f64::from),
        None => default,
    }
}

impl Drawer<'_> {
    /// Draw drawable `v` into `r`. Returns whether anything could be drawn.
    pub fn draw(&self, v: &Value, r: Rect) -> bool {
        self.draw_at(v, r, 0)
    }

    /// Draw an inline drawable of the app's (an adaptive icon's layer
    /// written in place) into `r`.
    pub fn draw_element(&self, e: &Element, r: Rect) -> bool {
        self.draw_xml(e, self.res.app, r, 1)
    }

    fn draw_at(&self, v: &Value, r: Rect, depth: u32) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let Some((v, apk)) = self.res.resolve(v) else {
            return false;
        };
        match v {
            Value::Color(c) => {
                self.canvas.fill_rect(r, c);
                true
            }
            Value::String(path) if path.ends_with(".xml") => match apk.file(&path) {
                Ok(b) => match crate::res::xml(&b) {
                    Ok(e) => self.draw_xml(&e, apk, r, depth + 1),
                    Err(_) => false,
                },
                Err(_) => false,
            },
            Value::String(path) => match apk.file(&path).ok().and_then(|b| Image::decode(&b)) {
                Some(img) => {
                    self.canvas.draw_image(&img, r);
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// The drawable an element names (`android:drawable`) or holds.
    fn inner(&self, e: &Element, apk: &Apk, r: Rect, depth: u32) -> bool {
        if let Some(v) = e.named("drawable") {
            return self.draw_at(v, r, depth);
        }
        e.children
            .first()
            .is_some_and(|c| self.draw_xml(c, apk, r, depth + 1))
    }

    fn draw_xml(&self, e: &Element, apk: &Apk, r: Rect, depth: u32) -> bool {
        match e.name.as_str() {
            "vector" => self.vector(e, r),
            "bitmap" | "nine-patch" => e.named("src").is_some_and(|v| self.draw_at(v, r, depth)),
            "color" => match e.named("color").and_then(|v| self.paint(v)) {
                Some(Paint::Solid(c)) => {
                    self.canvas.fill_rect(r, c);
                    true
                }
                _ => false,
            },
            "inset" => {
                let all = e.named("inset");
                let side = |name: &str, extent: f64| {
                    e.named(name)
                        .or(all)
                        .and_then(|v| self.length(v, extent))
                        .unwrap_or(0.0)
                };
                let inner = r.inset(
                    side("insetLeft", r.w),
                    side("insetTop", r.h),
                    side("insetRight", r.w),
                    side("insetBottom", r.h),
                );
                self.inner(e, apk, inner, depth)
            }
            "layer-list" | "ripple" => {
                let mut drew = false;
                for item in e.children.iter().filter(|c| c.name == "item") {
                    // A ripple's mask is not drawn.
                    if e.name == "ripple" && item.named("id").is_some() {
                        continue;
                    }
                    let side = |name: &str| {
                        item.named(name)
                            .and_then(|v| self.length(v, 0.0))
                            .unwrap_or(0.0)
                    };
                    let inner = r.inset(side("left"), side("top"), side("right"), side("bottom"));
                    drew |= self.inner(item, apk, inner, depth);
                }
                drew
            }
            "selector" | "level-list" => {
                // The item for no state: the last without state attributes.
                let items: Vec<&Element> = e.children.iter().filter(|c| c.name == "item").collect();
                let plain = items
                    .iter()
                    .rev()
                    .find(|i| !i.attrs.iter().any(|a| a.name.starts_with("state_")))
                    .or(items.first());
                plain.is_some_and(|i| self.inner(i, apk, r, depth))
            }
            "rotate" | "scale" | "clip" | "animated-vector" | "adaptive-icon" => {
                if e.name == "adaptive-icon" {
                    let mut drew = false;
                    for part in ["background", "foreground"] {
                        if let Some(p) = e.children.iter().find(|c| c.name == part) {
                            drew |= self.inner(p, apk, r, depth);
                        }
                    }
                    drew
                } else {
                    self.inner(e, apk, r, depth)
                }
            }
            "shape" => self.shape(e, r),
            _ => false,
        }
    }

    /// A length attribute in canvas pixels: a dimension in dp, or a
    /// fraction of `extent`.
    fn length(&self, v: &Value, extent: f64) -> Option<f64> {
        let (v, _) = self.res.resolve(v)?;
        match v {
            Value::Fraction(_) => v.length(extent as f32).map(f64::from),
            _ => v.length(1.0).map(|d| f64::from(d) * self.dp),
        }
    }

    /// A color or gradient value.
    fn paint(&self, v: &Value) -> Option<Paint> {
        let (v, apk) = self.res.resolve(v)?;
        match v {
            Value::Color(c) => Some(Paint::Solid(c)),
            Value::String(path) if path.ends_with(".xml") => {
                let e = crate::res::xml(&apk.file(&path).ok()?).ok()?;
                match e.name.as_str() {
                    "gradient" => self.gradient(&e).map(Paint::Gradient),
                    // A color state list: its color for no state.
                    "selector" => {
                        let item = e
                            .children
                            .iter()
                            .rev()
                            .find(|i| !i.attrs.iter().any(|a| a.name.starts_with("state_")))?;
                        let Paint::Solid(c) = self.paint(item.named("color")?)? else {
                            return None;
                        };
                        let alpha = item
                            .named("alpha")
                            .and_then(|a| a.length(1.0))
                            .unwrap_or(1.0);
                        Some(Paint::Solid(with_alpha(c, alpha as f64)))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A `<gradient>` (a complex color of a vector path), in the path's
    /// coordinates.
    fn gradient(&self, e: &Element) -> Option<Gradient> {
        let color = |name: &str| match e.named(name).and_then(|v| self.paint(v)) {
            Some(Paint::Solid(c)) => Some(c),
            _ => None,
        };
        let mut stops: Vec<(f64, u32)> = e
            .children
            .iter()
            .filter(|c| c.name == "item")
            .filter_map(|i| {
                let Some(Paint::Solid(c)) = i.named("color").and_then(|v| self.paint(v)) else {
                    return None;
                };
                Some((num(i, "offset", 0.0), c))
            })
            .collect();
        if stops.is_empty() {
            stops.push((0.0, color("startColor")?));
            if let Some(c) = color("centerColor") {
                stops.push((0.5, c));
            }
            stops.push((1.0, color("endColor")?));
        }
        Some(match e.named("type") {
            Some(Value::Int(1)) => Gradient::Radial {
                stops,
                center: [num(e, "centerX", 0.0), num(e, "centerY", 0.0)],
                radius: num(e, "gradientRadius", 0.0),
            },
            // Sweep gradients are drawn linear: close for an icon.
            _ => Gradient::Linear {
                stops,
                from: [num(e, "startX", 0.0), num(e, "startY", 0.0)],
                to: [num(e, "endX", 0.0), num(e, "endY", 0.0)],
            },
        })
    }

    fn shape(&self, e: &Element, r: Rect) -> bool {
        let mut path = Path::new();
        let child = |name: &str| e.children.iter().find(|c| c.name == name);
        match e.named("shape") {
            Some(Value::Int(1)) => path.ellipse(r),
            _ => {
                let radius = child("corners")
                    .and_then(|c| c.named("radius"))
                    .and_then(|v| self.length(v, 0.0))
                    .unwrap_or(0.0)
                    .min(r.w / 2.0)
                    .min(r.h / 2.0);
                rounded_rect(&mut path, r, radius);
            }
        }
        let mut drew = false;
        if let Some(solid) = child("solid").and_then(|s| s.named("color"))
            && let Some(Paint::Solid(c)) = self.paint(solid)
        {
            self.canvas.fill(&path, c, false);
            drew = true;
        }
        if let Some(g) = child("gradient") {
            let angle = num(g, "angle", 0.0).to_radians();
            let color = |name: &str| match g.named(name).and_then(|v| self.paint(v)) {
                Some(Paint::Solid(c)) => Some(c),
                _ => None,
            };
            if let (Some(a), Some(b)) = (color("startColor"), color("endColor")) {
                let mut stops = vec![(0.0, a)];
                if let Some(c) = color("centerColor") {
                    stops.push((0.5, c));
                }
                stops.push((1.0, b));
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                let (dx, dy) = (angle.cos() * r.w / 2.0, -angle.sin() * r.h / 2.0);
                let gradient = Gradient::Linear {
                    stops,
                    from: [cx - dx, cy - dy],
                    to: [cx + dx, cy + dy],
                };
                self.canvas.fill_gradient(&path, &gradient, false);
                drew = true;
            }
        }
        drew
    }

    /// A `<vector>`: its viewport scaled onto `r`.
    fn vector(&self, e: &Element, r: Rect) -> bool {
        let vw = num(e, "viewportWidth", 24.0);
        let vh = num(e, "viewportHeight", 24.0);
        if vw <= 0.0 || vh <= 0.0 {
            return false;
        }
        let tint = e.named("tint").and_then(|v| match self.paint(v) {
            Some(Paint::Solid(c)) => Some(c),
            _ => None,
        });
        let draw = || {
            self.canvas.saved(|| {
                self.canvas.alpha(num(e, "alpha", 1.0));
                self.canvas.transform(Transform {
                    a: r.w / vw,
                    b: 0.0,
                    c: 0.0,
                    d: r.h / vh,
                    tx: r.x,
                    ty: r.y,
                });
                self.group(e);
            })
        };
        match tint {
            Some(t) => self.canvas.tinted(t, r, draw),
            None => draw(),
        }
        true
    }

    /// A group's children in order; a clip path clips what follows it.
    fn group(&self, g: &Element) {
        self.canvas.saved(|| {
            for c in &g.children {
                match c.name.as_str() {
                    "group" => self.canvas.saved(|| {
                        let (px, py) = (num(c, "pivotX", 0.0), num(c, "pivotY", 0.0));
                        let (sx, sy) = (num(c, "scaleX", 1.0), num(c, "scaleY", 1.0));
                        let rot = num(c, "rotation", 0.0).to_radians();
                        let (tx, ty) = (num(c, "translateX", 0.0), num(c, "translateY", 0.0));
                        let (cos, sin) = (rot.cos(), rot.sin());
                        // translate(t + p) · rotate · scale · translate(-p)
                        let (a, b, cc, d) = (cos * sx, sin * sx, -sin * sy, cos * sy);
                        self.canvas.transform(Transform {
                            a,
                            b,
                            c: cc,
                            d,
                            tx: tx + px - (a * px + cc * py),
                            ty: ty + py - (b * px + d * py),
                        });
                        self.group(c);
                    }),
                    "clip-path" => {
                        if let Some(p) = path_of(c) {
                            self.canvas.clip(&p, false);
                        }
                    }
                    "path" => self.path(c),
                    _ => {}
                }
            }
        });
    }

    fn path(&self, e: &Element) {
        let Some(p) = path_of(e) else { return };
        let even_odd = matches!(e.named("fillType"), Some(Value::Int(1)));
        if let Some(fill) = e.named("fillColor").and_then(|v| self.paint(v)) {
            self.canvas.saved(|| {
                self.canvas.alpha(num(e, "fillAlpha", 1.0));
                match &fill {
                    Paint::Solid(c) => self.canvas.fill(&p, *c, even_odd),
                    Paint::Gradient(g) => self.canvas.fill_gradient(&p, g, even_odd),
                }
            });
        }
        if let Some(Paint::Solid(c)) = e.named("strokeColor").and_then(|v| self.paint(v)) {
            let width = num(e, "strokeWidth", 0.0);
            if width > 0.0 {
                let int = |name: &str| match e.named(name) {
                    Some(Value::Int(i)) => *i,
                    _ => 0,
                };
                self.canvas.saved(|| {
                    self.canvas.alpha(num(e, "strokeAlpha", 1.0));
                    self.canvas.stroke(
                        &p,
                        c,
                        Stroke {
                            width,
                            cap: int("strokeLineCap"),
                            join: int("strokeLineJoin"),
                            miter: num(e, "strokeMiterLimit", 4.0),
                        },
                    );
                });
            }
        }
    }
}

fn with_alpha(argb: u32, alpha: f64) -> u32 {
    let a = ((argb >> 24) as f64 * alpha.clamp(0.0, 1.0)).round() as u32;
    (argb & 0x00ff_ffff) | a << 24
}

fn rounded_rect(p: &mut Path, r: Rect, radius: f64) {
    let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
    // A quarter circle's cubic control distance.
    let k = radius * 0.552_284_75;
    p.move_to(x0 + radius, y0);
    p.line_to(x1 - radius, y0);
    p.cubic_to(x1 - radius + k, y0, x1, y0 + radius - k, x1, y0 + radius);
    p.line_to(x1, y1 - radius);
    p.cubic_to(x1, y1 - radius + k, x1 - radius + k, y1, x1 - radius, y1);
    p.line_to(x0 + radius, y1);
    p.cubic_to(x0 + radius - k, y1, x0, y1 - radius + k, x0, y1 - radius);
    p.line_to(x0, y0 + radius);
    p.cubic_to(x0, y0 + radius - k, x0 + radius - k, y0, x0 + radius, y0);
    p.close();
}

fn path_of(e: &Element) -> Option<Path> {
    let Some(Value::String(d)) = e.named("pathData") else {
        return None;
    };
    let mut p = Path::new();
    crate::pathdata::build(d, &mut p);
    Some(p)
}
