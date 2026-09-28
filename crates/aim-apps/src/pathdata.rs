//! Vector drawable path data (`android:pathData`): SVG's path syntax, as
//! Android's `PathParser` reads it, into cubic, quadratic and straight
//! segments. Arcs become cubic Béziers (SVG 1.1, F.6).

use crate::cg::Path;

/// Where `build` sends the segments.
pub trait Sink {
    fn move_to(&mut self, x: f64, y: f64);
    fn line_to(&mut self, x: f64, y: f64);
    fn cubic_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64);
    fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64);
    fn close(&mut self);
}

impl Sink for Path {
    fn move_to(&mut self, x: f64, y: f64) {
        Path::move_to(self, x, y)
    }
    fn line_to(&mut self, x: f64, y: f64) {
        Path::line_to(self, x, y)
    }
    fn cubic_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        Path::cubic_to(self, x1, y1, x2, y2, x, y)
    }
    fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
        Path::quad_to(self, cx, cy, x, y)
    }
    fn close(&mut self) {
        Path::close(self)
    }
}

/// The numbers after a command: separators are commas and white space,
/// and a sign or a second decimal point starts a new number.
fn numbers(s: &str, arc: bool) -> Vec<f64> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && (b[i] == b',' || b[i].is_ascii_whitespace()) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        // An arc's flags are single digits, often run together.
        if arc && matches!(out.len() % 7, 3 | 4) && (b[i] == b'0' || b[i] == b'1') {
            out.push(f64::from(b[i] - b'0'));
            i += 1;
            continue;
        }
        let start = i;
        if b[i] == b'-' || b[i] == b'+' {
            i += 1;
        }
        let mut dot = false;
        let mut exp = false;
        while i < b.len() {
            match b[i] {
                b'0'..=b'9' => {}
                b'.' if !dot && !exp => dot = true,
                b'e' | b'E' if !exp => {
                    exp = true;
                    if i + 1 < b.len() && (b[i + 1] == b'-' || b[i + 1] == b'+') {
                        i += 1;
                    }
                }
                _ => break,
            }
            i += 1;
        }
        match s[start..i].parse() {
            Ok(v) => out.push(v),
            Err(_) => i = start + 1,
        }
    }
    out
}

/// Build path data `d` into `p`.
pub fn build(d: &str, p: &mut impl Sink) {
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    // The last control point, for S and T.
    let mut ctrl: Option<(f64, f64)> = None;
    let mut last = b' ';
    let bytes = d.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let cmd = bytes[i];
        if !cmd.is_ascii_alphabetic() || cmd == b'e' || cmd == b'E' {
            i += 1;
            continue;
        }
        let end = bytes[i + 1..]
            .iter()
            .position(|c| c.is_ascii_alphabetic() && *c != b'e' && *c != b'E')
            .map_or(bytes.len(), |n| i + 1 + n);
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        let args = numbers(&d[i + 1..end], up == b'A');
        i = end;
        let arity = match up {
            b'M' | b'L' | b'T' => 2,
            b'H' | b'V' => 1,
            b'C' => 6,
            b'S' | b'Q' => 4,
            b'A' => 7,
            b'Z' => 0,
            _ => continue,
        };
        if up == b'Z' {
            p.close();
            (cx, cy) = (sx, sy);
            ctrl = None;
            last = up;
            continue;
        }
        for (k, a) in args.chunks_exact(arity).enumerate() {
            let (ox, oy) = if rel { (cx, cy) } else { (0.0, 0.0) };
            match up {
                b'M' if k == 0 => {
                    (cx, cy) = (ox + a[0], oy + a[1]);
                    (sx, sy) = (cx, cy);
                    p.move_to(cx, cy);
                    ctrl = None;
                }
                // Pairs after a move are lines.
                b'M' | b'L' => {
                    (cx, cy) = (ox + a[0], oy + a[1]);
                    p.line_to(cx, cy);
                    ctrl = None;
                }
                b'H' => {
                    cx = if rel { cx + a[0] } else { a[0] };
                    p.line_to(cx, cy);
                    ctrl = None;
                }
                b'V' => {
                    cy = if rel { cy + a[0] } else { a[0] };
                    p.line_to(cx, cy);
                    ctrl = None;
                }
                b'C' => {
                    let (x2, y2) = (ox + a[2], oy + a[3]);
                    (cx, cy) = (ox + a[4], oy + a[5]);
                    p.cubic_to(ox + a[0], oy + a[1], x2, y2, cx, cy);
                    ctrl = Some((x2, y2));
                }
                b'S' => {
                    let (x1, y1) = match (ctrl, matches!(last, b'C' | b'S')) {
                        (Some((qx, qy)), true) => (2.0 * cx - qx, 2.0 * cy - qy),
                        _ => (cx, cy),
                    };
                    let (x2, y2) = (ox + a[0], oy + a[1]);
                    (cx, cy) = (ox + a[2], oy + a[3]);
                    p.cubic_to(x1, y1, x2, y2, cx, cy);
                    ctrl = Some((x2, y2));
                }
                b'Q' => {
                    let (qx, qy) = (ox + a[0], oy + a[1]);
                    (cx, cy) = (ox + a[2], oy + a[3]);
                    p.quad_to(qx, qy, cx, cy);
                    ctrl = Some((qx, qy));
                }
                b'T' => {
                    let (qx, qy) = match (ctrl, matches!(last, b'Q' | b'T')) {
                        (Some((qx, qy)), true) => (2.0 * cx - qx, 2.0 * cy - qy),
                        _ => (cx, cy),
                    };
                    (cx, cy) = (ox + a[0], oy + a[1]);
                    p.quad_to(qx, qy, cx, cy);
                    ctrl = Some((qx, qy));
                }
                b'A' => {
                    let (x, y) = (ox + a[5], oy + a[6]);
                    arc(
                        p,
                        (cx, cy),
                        (a[0], a[1]),
                        a[2],
                        a[3] != 0.0,
                        a[4] != 0.0,
                        (x, y),
                    );
                    (cx, cy) = (x, y);
                    ctrl = None;
                }
                _ => {}
            }
            last = up;
        }
    }
}

/// An elliptical arc from `from` to `to` as cubic Béziers.
fn arc(
    p: &mut impl Sink,
    from: (f64, f64),
    radii: (f64, f64),
    rotation: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
) {
    let (x1, y1) = from;
    let (x2, y2) = to;
    let (mut rx, mut ry) = (radii.0.abs(), radii.1.abs());
    if rx == 0.0 || ry == 0.0 || (x1 == x2 && y1 == y2) {
        p.line_to(x2, y2);
        return;
    }
    let phi = rotation.to_radians();
    let (cos, sin) = (phi.cos(), phi.sin());
    let dx = (x1 - x2) / 2.0;
    let dy = (y1 - y2) / 2.0;
    let x1p = cos * dx + sin * dy;
    let y1p = -sin * dx + cos * dy;
    // Radii too small to reach are scaled up.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = (num / den).max(0.0).sqrt();
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    let cx = cos * cxp - sin * cyp + (x1 + x2) / 2.0;
    let cy = sin * cxp + cos * cyp + (y1 + y2) / 2.0;
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        if a.is_nan() { 0.0 } else { a }
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut delta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && delta > 0.0 {
        delta -= std::f64::consts::TAU;
    } else if sweep && delta < 0.0 {
        delta += std::f64::consts::TAU;
    }
    let n = (delta.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = delta / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| {
        let (ct, st) = (t.cos(), t.sin());
        (
            cx + rx * ct * cos - ry * st * sin,
            cy + rx * ct * sin + ry * st * cos,
        )
    };
    let deriv = |t: f64| {
        let (ct, st) = (t.cos(), t.sin());
        (
            -rx * st * cos - ry * ct * sin,
            -rx * st * sin + ry * ct * cos,
        )
    };
    let mut t = theta1;
    for i in 0..n {
        let t2 = t + step;
        let (ax, ay) = point(t);
        let (bx, by) = if i + 1 == n { (x2, y2) } else { point(t2) };
        let (dax, day) = deriv(t);
        let (dbx, dby) = deriv(t2);
        p.cubic_to(
            ax + k * dax,
            ay + k * day,
            bx - k * dbx,
            by - k * dby,
            bx,
            by,
        );
        t = t2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec(Vec<String>);

    impl Sink for Rec {
        fn move_to(&mut self, x: f64, y: f64) {
            self.0.push(format!("M{x} {y}"));
        }
        fn line_to(&mut self, x: f64, y: f64) {
            self.0.push(format!("L{x} {y}"));
        }
        fn cubic_to(&mut self, _: f64, _: f64, _: f64, _: f64, x: f64, y: f64) {
            self.0.push(format!("C{:.3} {:.3}", x, y));
        }
        fn quad_to(&mut self, cx: f64, cy: f64, x: f64, y: f64) {
            self.0.push(format!("Q{cx} {cy} {x} {y}"));
        }
        fn close(&mut self) {
            self.0.push("Z".into());
        }
    }

    fn run(d: &str) -> Vec<String> {
        let mut r = Rec::default();
        build(d, &mut r);
        r.0
    }

    #[test]
    fn commands_and_run_together_numbers() {
        assert_eq!(
            run("M10,10h5v-5l-1-1.5.5.5zm1 1"),
            [
                "M10 10", "L15 10", "L15 5", "L14 3.5", "L14.5 4", "Z", "M11 11"
            ]
        );
        // Pairs after a move are lines; relative ones from the last point.
        assert_eq!(run("m1 1 2 2 3 3"), ["M1 1", "L3 3", "L6 6"]);
        assert_eq!(
            run("M0 0Q5 5 10 0T20 0"),
            ["M0 0", "Q5 5 10 0", "Q15 -5 20 0"]
        );
        assert_eq!(run("M1e1 2E-1L-1e+1-2"), ["M10 0.2", "L-10 -2"]);
    }

    #[test]
    fn arcs_end_where_they_should() {
        // A half circle of radius 5 as two quarter segments; flags run
        // together as Android's parser allows.
        let r = run("M0 5a5 5 0 01 10 0");
        assert_eq!(r, ["M0 5", "C5.000 0.000", "C10.000 5.000"]);
        assert_eq!(run("M0 0A0 0 0 0 1 3 4"), ["M0 0", "L3 4"]);
    }
}
