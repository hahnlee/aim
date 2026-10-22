//! Drawing an app's drawables the way Android draws them, for its icon:
//! colors, bitmaps (PNG, WebP), vector drawables with their groups, clip
//! paths, strokes and gradients, and the containers launcher icons use
//! (`inset`, `rotate`, `layer-list`, `bitmap`, `shape`, `selector`, ...),
//! with levels as Android sets them (0 unless a `scale` sets one), so a
//! `clip` or `scale` at level 0 draws nothing.
//!
//! Lengths in dp are drawn at the canvas's scale for the drawable's frame;
//! theme attributes resolve against the app's theme.

use crate::apk::{Apk, Resources};
use crate::cg::{Canvas, Gradient, Image, Path, Rect, Stroke, Transform};
use crate::res::{Element, Value};

/// How deep drawables may nest (a drawable referring to itself ends).
const MAX_DEPTH: u32 = 16;

/// A drawable's intrinsic width and height, in canvas pixels.
type Size = (Option<f64>, Option<f64>);

/// A drawable's highest level (`Drawable.MAX_LEVEL`).
const MAX_LEVEL: f64 = 10000.0;

/// `Gravity.LEFT`, the default of `scale` and `clip`.
const LEFT: i32 = 3;

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
        self.draw_at(v, r, 0, 0)
    }

    /// Draw an inline drawable of the app's (an adaptive icon's layer
    /// written in place) into `r`.
    pub fn draw_element(&self, e: &Element, r: Rect) -> bool {
        self.draw_xml(e, self.res.app, r, 1, 0)
    }

    /// Draw `v` at `level` (0 to 10000; a drawable's is 0 unless a
    /// container sets it).
    fn draw_at(&self, v: &Value, r: Rect, depth: u32, level: u32) -> bool {
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
                    Ok(e) => self.draw_xml(&e, apk, r, depth + 1, level),
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
    fn inner(&self, e: &Element, apk: &Apk, r: Rect, depth: u32, level: u32) -> bool {
        if let Some(v) = e.named("drawable") {
            return self.draw_at(v, r, depth, level);
        }
        e.children
            .first()
            .is_some_and(|c| self.draw_xml(c, apk, r, depth + 1, level))
    }

    /// `Drawable.getIntrinsicWidth` and `getIntrinsicHeight` of drawable
    /// `v`, in canvas pixels; `None` where Android answers -1.
    fn intrinsic(&self, v: &Value, depth: u32) -> Size {
        if depth > MAX_DEPTH {
            return (None, None);
        }
        let Some((v, apk)) = self.res.resolve(v) else {
            return (None, None);
        };
        match v {
            Value::String(path) if path.ends_with(".xml") => apk
                .file(&path)
                .ok()
                .and_then(|b| crate::res::xml(&b).ok())
                .map_or((None, None), |e| self.intrinsic_xml(&e, apk, depth + 1)),
            // A bitmap's pixels at its directory's density, in dp.
            Value::String(path) => match (
                apk.file(&path).ok().and_then(|b| Image::decode(&b)),
                density(&path),
            ) {
                (Some(img), Some(density)) => {
                    let (w, h) = img.size();
                    let px = self.dp * 160.0 / density;
                    (Some(w as f64 * px), Some(h as f64 * px))
                }
                _ => (None, None),
            },
            _ => (None, None),
        }
    }

    /// The intrinsic size of the drawable an element names or holds.
    fn intrinsic_inner(&self, e: &Element, apk: &Apk, depth: u32) -> Size {
        if let Some(v) = e.named("drawable") {
            return self.intrinsic(v, depth);
        }
        e.children
            .first()
            .map_or((None, None), |c| self.intrinsic_xml(c, apk, depth + 1))
    }

    fn intrinsic_xml(&self, e: &Element, apk: &Apk, depth: u32) -> Size {
        if depth > MAX_DEPTH {
            return (None, None);
        }
        let attr = |e: &Element, name: &str| e.named(name).and_then(|v| self.length(v, 0.0));
        match e.name.as_str() {
            "vector" => (attr(e, "width"), attr(e, "height")),
            "bitmap" | "nine-patch" => e
                .named("src")
                .map_or((None, None), |v| self.intrinsic(v, depth)),
            "shape" => e
                .children
                .iter()
                .find(|c| c.name == "size")
                .map_or((None, None), |size| {
                    (attr(size, "width"), attr(size, "height"))
                }),
            "inset" => {
                // InsetDrawable: the child's size grown by the insets
                // (fractions of the size, then dimensions).
                let (cw, ch) = self.intrinsic_inner(e, apk, depth);
                let all = e.named("inset");
                let inset = |name: &str| match e.named(name).or(all) {
                    Some(v) => match self.res.resolve(v) {
                        Some((Value::Fraction(f), _)) => (f64::from(f), 0.0),
                        _ => (0.0, self.length(v, 0.0).unwrap_or(0.0)),
                    },
                    None => (0.0, 0.0),
                };
                let axis = |child: Option<f64>, a: &str, b: &str| {
                    let ((fa, da), (fb, db)) = (inset(a), inset(b));
                    let child = child?;
                    (fa + fb < 1.0).then(|| child / (1.0 - (fa + fb)) + da + db)
                };
                (
                    axis(cw, "insetLeft", "insetRight"),
                    axis(ch, "insetTop", "insetBottom"),
                )
            }
            "layer-list" | "ripple" => {
                // LayerDrawable: the largest of the layers with a size,
                // each with its insets.
                let mut size: Size = (None, None);
                for item in e.children.iter().filter(|c| c.name == "item") {
                    if e.name == "ripple" && item.named("id").is_some() {
                        continue;
                    }
                    let (cw, ch) = self.intrinsic_inner(item, apk, depth);
                    let side = |name: &str| attr(item, name).unwrap_or(0.0);
                    let w = attr(item, "width")
                        .or(cw)
                        .map(|w| w + side("left") + side("right"));
                    let h = attr(item, "height")
                        .or(ch)
                        .map(|h| h + side("top") + side("bottom"));
                    let larger = |a: Option<f64>, b: Option<f64>| match (a, b) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                    size = (larger(size.0, w), larger(size.1, h));
                }
                size
            }
            "selector" => {
                selector_item(e).map_or((None, None), |i| self.intrinsic_inner(i, apk, depth))
            }
            "animation-list" => e
                .children
                .iter()
                .find(|c| c.name == "item")
                .map_or((None, None), |i| self.intrinsic_inner(i, apk, depth)),
            "level-list" => self
                .level_item(e, 0)
                .map_or((None, None), |i| self.intrinsic_inner(i, apk, depth)),
            // DrawableWrapper: its child's.
            "rotate" | "scale" | "clip" | "animated-vector" => self.intrinsic_inner(e, apk, depth),
            _ => (None, None),
        }
    }

    /// The first item of a `level-list` whose level range holds `level`.
    fn level_item<'e>(&self, e: &'e Element, level: u32) -> Option<&'e Element> {
        let level = level as i32;
        e.children
            .iter()
            .filter(|c| c.name == "item")
            .find(|i| (self.int(i, "minLevel", 0)..=self.int(i, "maxLevel", 0)).contains(&level))
    }

    /// A boolean attribute (false unless set).
    fn boolean(&self, e: &Element, name: &str) -> bool {
        matches!(
            e.named(name).and_then(|v| self.res.resolve(v)),
            Some((Value::Bool(true), _))
        )
    }

    /// An integer attribute.
    fn int(&self, e: &Element, name: &str, default: i32) -> i32 {
        match e.named(name).and_then(|v| self.res.resolve(v)) {
            Some((Value::Int(i), _)) => i,
            _ => default,
        }
    }

    fn draw_xml(&self, e: &Element, apk: &Apk, r: Rect, depth: u32, level: u32) -> bool {
        match e.name.as_str() {
            "vector" => self.vector(e, r),
            "bitmap" | "nine-patch" => e
                .named("src")
                .is_some_and(|v| self.draw_at(v, r, depth, level)),
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
                self.inner(e, apk, inner, depth, level)
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
                    drew |= self.inner(item, apk, inner, depth, level);
                }
                drew
            }
            "selector" => selector_item(e).is_some_and(|i| self.inner(i, apk, r, depth, level)),
            "animation-list" => {
                // An AnimationDrawable not running shows its first frame
                // (a status icon such as the download arrow).
                e.children
                    .iter()
                    .find(|c| c.name == "item")
                    .is_some_and(|i| self.inner(i, apk, r, depth, level))
            }
            "level-list" => self
                .level_item(e, level)
                .is_some_and(|i| self.inner(i, apk, r, depth, level)),
            "rotate" => {
                // It stands between `fromDegrees` and `toDegrees` by its
                // level, about its pivot (50 % by default).
                let pivot = |name: &str, extent: f64| match e.named(name) {
                    Some(v) => self.res.resolve(v).and_then(|(v, _)| match v {
                        Value::Fraction(_) => v.length(extent as f32).map(f64::from),
                        v => v.length(1.0).map(f64::from),
                    }),
                    None => Some(extent / 2.0),
                };
                let (Some(px), Some(py)) = (pivot("pivotX", r.w), pivot("pivotY", r.h)) else {
                    return false;
                };
                let from = num(e, "fromDegrees", 0.0);
                let to = num(e, "toDegrees", 360.0);
                let degrees = from + (to - from) * f64::from(level) / MAX_LEVEL;
                let t = rotation(degrees, r.x + px, r.y + py);
                let mut drew = false;
                self.canvas.saved(|| {
                    self.canvas.transform(t);
                    drew = self.inner(e, apk, r, depth, level);
                });
                drew
            }
            "scale" => {
                // `android:level` sets its level, and so its child's; at
                // level 0 the child is not drawn.
                let level = self.int(e, "level", level as i32).clamp(0, 10000) as u32;
                if level == 0 {
                    return false;
                }
                // With `useIntrinsicSizeAsMinimum`, the child shrinks down
                // to its intrinsic size, not to nothing (`onBoundsChange`).
                let (iw, ih) = if self.boolean(e, "useIntrinsicSizeAsMinimum") {
                    self.intrinsic_inner(e, apk, depth)
                } else {
                    (None, None)
                };
                let scale =
                    |name: &str, extent: f64, minimum: Option<f64>| match self.percent(e, name) {
                        Some(s) if s > 0.0 => scaled(extent, minimum.unwrap_or(0.0), level, s),
                        _ => extent,
                    };
                let (w, h) = (scale("scaleWidth", r.w, iw), scale("scaleHeight", r.h, ih));
                if w <= 0.0 || h <= 0.0 {
                    return false;
                }
                let inner = gravity(self.int(e, "scaleGravity", LEFT), w, h, r);
                self.inner(e, apk, inner, depth, level)
            }
            "clip" => {
                // It shows the part of its child its level gives (none at
                // level 0), along its orientation (horizontal by default).
                if level == 0 {
                    return false;
                }
                let orientation = self.int(e, "clipOrientation", 1);
                let w = if orientation & 1 != 0 {
                    at_level(r.w, level, 1.0)
                } else {
                    r.w
                };
                let h = if orientation & 2 != 0 {
                    at_level(r.h, level, 1.0)
                } else {
                    r.h
                };
                let shown = gravity(self.int(e, "gravity", LEFT), w, h, r);
                let mut clip = Path::new();
                clip.move_to(shown.x, shown.y);
                clip.line_to(shown.x + shown.w, shown.y);
                clip.line_to(shown.x + shown.w, shown.y + shown.h);
                clip.line_to(shown.x, shown.y + shown.h);
                clip.close();
                let mut drew = false;
                self.canvas.saved(|| {
                    self.canvas.clip(&clip, false);
                    drew = self.inner(e, apk, r, depth, level);
                });
                drew
            }
            "animated-vector" => self.inner(e, apk, r, depth, level),
            "adaptive-icon" => {
                let mut drew = false;
                for part in ["background", "foreground"] {
                    if let Some(p) = e.children.iter().find(|c| c.name == part) {
                        drew |= self.inner(p, apk, r, depth, level);
                    }
                }
                drew
            }
            "shape" => self.shape(e, r),
            _ => false,
        }
    }

    /// A scale attribute: a fraction, or a string such as `"50%"`.
    fn percent(&self, e: &Element, name: &str) -> Option<f64> {
        match self.res.resolve(e.named(name)?)?.0 {
            v @ Value::Fraction(_) => v.length(1.0).map(f64::from),
            Value::String(s) => s
                .strip_suffix('%')?
                .trim()
                .parse::<f64>()
                .ok()
                .map(|p| p / 100.0),
            _ => None,
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

/// A `selector`'s item for no state: the last without state attributes.
fn selector_item(e: &Element) -> Option<&Element> {
    let items: Vec<&Element> = e.children.iter().filter(|c| c.name == "item").collect();
    items
        .iter()
        .rev()
        .find(|i| !i.attrs.iter().any(|a| a.name.starts_with("state_")))
        .or(items.first())
        .copied()
}

/// A `ScaleDrawable`'s extent: shrunk from `extent` towards `minimum` by
/// `scale` of the difference at level 0, whole at 10000.
fn scaled(extent: f64, minimum: f64, level: u32, scale: f64) -> f64 {
    extent - (extent - minimum) * (1.0 - f64::from(level) / MAX_LEVEL) * scale
}

/// The density of a bitmap's resource directory (`res/drawable-xhdpi`):
/// 160 without a density qualifier; none for `nodpi`, whose pixels are
/// the device's, which the drawer does not know.
fn density(path: &str) -> Option<f64> {
    let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
    let dir = dir.rsplit('/').next().unwrap_or(dir);
    for q in dir.split('-').skip(1) {
        let dpi = match q {
            "ldpi" => 120.0,
            "mdpi" => 160.0,
            "tvdpi" => 213.0,
            "hdpi" => 240.0,
            "xhdpi" => 320.0,
            "xxhdpi" => 480.0,
            "xxxhdpi" => 640.0,
            "nodpi" => return None,
            q => match q.strip_suffix("dpi").and_then(|n| n.parse::<f64>().ok()) {
                Some(dpi) => dpi,
                None => continue,
            },
        };
        return Some(dpi);
    }
    Some(160.0)
}

/// An extent scaled down by `scale` of it at level 0, whole at 10000.
fn at_level(extent: f64, level: u32, scale: f64) -> f64 {
    extent - extent * (1.0 - f64::from(level) / MAX_LEVEL) * scale
}

/// `Gravity.apply` (left to right): a `w` by `h` rect placed in `r`.
fn gravity(g: i32, w: f64, h: f64, r: Rect) -> Rect {
    // Per axis: 2 pulls to the start, 4 to the end, both fill, neither
    // centers.
    let axis = |bits: i32, start: f64, extent: f64, size: f64| match bits & 6 {
        0 => (start + (extent - size) / 2.0, size),
        2 => (start, size),
        4 => (start + extent - size, size),
        _ => (start, extent),
    };
    let (x, w) = axis(g, r.x, r.w, w);
    let (y, h) = axis(g >> 4, r.y, r.h, h);
    Rect::new(x, y, w, h)
}

/// A rotation by `degrees` (clockwise, y running down) about (px, py).
fn rotation(degrees: f64, px: f64, py: f64) -> Transform {
    let (sin, cos) = degrees.to_radians().sin_cos();
    Transform {
        a: cos,
        b: sin,
        c: -sin,
        d: cos,
        tx: px - (cos * px - sin * py),
        ty: py - (sin * px + cos * py),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_by_level() {
        // A clip shows nothing at level 0 and all at 10000; a 50 % scale
        // keeps half at level 0 and three quarters at 5000.
        assert_eq!(at_level(108.0, 0, 1.0), 0.0);
        assert_eq!(at_level(108.0, 10000, 1.0), 108.0);
        assert_eq!(at_level(108.0, 0, 0.5), 54.0);
        assert_eq!(at_level(108.0, 5000, 0.5), 81.0);
    }

    #[test]
    fn scale_keeps_the_intrinsic_size_as_its_minimum() {
        // A 50 % scale of 108 with a 48 intrinsic: 78 at level 0, half way
        // between at 5000, whole at 10000; without a minimum, as at_level.
        assert_eq!(scaled(108.0, 48.0, 0, 0.5), 78.0);
        assert_eq!(scaled(108.0, 48.0, 5000, 0.5), 93.0);
        assert_eq!(scaled(108.0, 48.0, 10000, 0.5), 108.0);
        assert_eq!(scaled(108.0, 0.0, 5000, 0.5), at_level(108.0, 5000, 0.5));
    }

    #[test]
    fn bitmap_density_from_its_directory() {
        assert_eq!(density("res/drawable-xxhdpi-v4/icon.png"), Some(480.0));
        assert_eq!(density("res/mipmap-hdpi/ic.webp"), Some(240.0));
        assert_eq!(density("res/drawable/icon.png"), Some(160.0));
        assert_eq!(density("res/drawable-560dpi/icon.png"), Some(560.0));
        assert_eq!(density("res/drawable-nodpi/icon.png"), None);
        assert_eq!(density("res/drawable-night-xhdpi/icon.png"), Some(320.0));
    }

    #[test]
    fn gravity_places() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        let at = |g| {
            let p = gravity(g, 40.0, 10.0, r);
            (p.x, p.y, p.w, p.h)
        };
        // LEFT: at the left, centered vertically.
        assert_eq!(at(0x03), (10.0, 40.0, 40.0, 10.0));
        // RIGHT | BOTTOM, CENTER, FILL, START (left to right).
        assert_eq!(at(0x55), (70.0, 60.0, 40.0, 10.0));
        assert_eq!(at(0x11), (40.0, 40.0, 40.0, 10.0));
        assert_eq!(at(0x77), (10.0, 20.0, 100.0, 50.0));
        assert_eq!(at(0x0080_0003 | 0x30), (10.0, 20.0, 40.0, 10.0));
    }

    #[test]
    fn rotation_about_pivot() {
        // A hand from the center (54, 54) to 12 o'clock, turned 90
        // degrees, points to 3 o'clock; the pivot stays.
        let t = rotation(90.0, 54.0, 54.0);
        let at = |x: f64, y: f64| (t.a * x + t.c * y + t.tx, t.b * x + t.d * y + t.ty);
        let (x, y) = at(54.0, 10.0);
        assert!((x - 98.0).abs() < 1e-9 && (y - 54.0).abs() < 1e-9);
        let (x, y) = at(54.0, 54.0);
        assert!((x - 54.0).abs() < 1e-9 && (y - 54.0).abs() < 1e-9);
    }
}
