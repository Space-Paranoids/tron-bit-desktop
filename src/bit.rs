//! Bit's shapes, and a small software renderer that draws them: flat
//! facets in a deep color, a sharp white glint, tumbling, the neutral ball
//! shimmering between its two forms, and each change of shape swelling out
//! through the old one.
//!
//! Output is premultiplied BGRA (`0xAARRGGBB`), antialiased by
//! supersampling, ready for a per-pixel-alpha window.

use std::f32::consts::TAU;
use std::sync::OnceLock;

type V3 = [f32; 3];
type Tri = [V3; 3];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Neutral,
    Yes,
    No,
}

/// Neutral's back-and-forth swaps between its two forms per second.
const SHIMMER: f32 = 1.6;
/// Seconds Bit takes to change shape.
pub const MORPH: f32 = 0.3;
/// Scale of a shape tucked away inside another (small enough to hide).
const TUCKED: f32 = 0.5;
/// Bit's colors, rich as in the film: icy blue, amber-yellow and orange-red.
const NEUTRAL: V3 = [0.48, 0.74, 1.0];
const YES: V3 = [1.0, 0.66, 0.0];
const NO: V3 = [1.0, 0.3, 0.0];
const YES_SIZE: f32 = 1.05;
/// The "speaking" pulse.
const PULSE: f32 = 0.06;

struct Shapes {
    /// Neutral flickers between two forms, as in the film.
    neutral: [Vec<Tri>; 2],
    yes: Vec<Tri>,
    no: Vec<Tri>,
    /// Furthest any shape reaches from the center (the neutral ball is 1).
    reach: f32,
    /// Furthest the neutral shape reaches.
    neutral_reach: f32,
}

/// `assets/bit.bin`: "BIT1", then for neutral, neutral 2, yes and
/// no a u32 triangle count and that many triangles (9 x f32, wound outward),
/// all at one scale so the neutral ball has radius 1.
fn shapes() -> &'static Shapes {
    static SHAPES: OnceLock<Shapes> = OnceLock::new();
    SHAPES.get_or_init(|| {
        let data: &[u8] = include_bytes!("../assets/bit.bin");
        assert_eq!(&data[..4], b"BIT1");
        let f32_at = |o: usize| f32::from_le_bytes(data[o..o + 4].try_into().unwrap());
        let mut at = 4;
        let mut next = || {
            let n = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            let tris: Vec<Tri> = (0..n)
                .map(|i| {
                    let o = at + i * 36;
                    [0, 1, 2].map(|k| {
                        [
                            f32_at(o + k * 12),
                            f32_at(o + k * 12 + 4),
                            f32_at(o + k * 12 + 8),
                        ]
                    })
                })
                .collect();
            at += n * 36;
            tris
        };
        let neutral = [next(), next()];
        let (yes, no) = (next(), next());
        let far = |tris: &[Tri]| tris.iter().flatten().map(|v| len(*v)).fold(0.0, f32::max);
        let neutral_reach = far(&neutral[0]).max(far(&neutral[1]));
        let reach = neutral_reach.max(far(&yes) * YES_SIZE).max(far(&no));
        Shapes {
            neutral,
            yes,
            no,
            reach,
            neutral_reach,
        }
    })
}

/// How far Bit can reach from its center, in units of its radius, pulse
/// included: a bitmap `2 * r * reach()` across always holds it.
pub fn reach() -> f32 {
    shapes().reach * (1.0 + PULSE)
}

/// How far the neutral shape reaches, like `reach` (the yes and no shapes
/// reach further).
pub fn neutral_reach() -> f32 {
    shapes().neutral_reach * (1.0 + PULSE)
}

pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of neutral's first form is out at time `t` (0 to 1).
fn neutral_mix(t: f32) -> f32 {
    smoothstep(0.15, 0.85, 0.5 + 0.5 * (t * TAU * SHIMMER).sin())
}

/// The shapes making up `mood` at time `t`, each with its scale and color.
/// Neutral is both of its forms at once, one swelling out through the other.
fn parts(mood: Mood, t: f32) -> Vec<(&'static [Tri], f32, V3)> {
    let s = shapes();
    match mood {
        Mood::Neutral => {
            let w = neutral_mix(t);
            let grow = |w: f32| TUCKED + (1.0 - TUCKED) * (2.0 * w).min(1.0);
            vec![
                (&s.neutral[0][..], grow(w), NEUTRAL),
                (&s.neutral[1][..], grow(1.0 - w), NEUTRAL),
            ]
        }
        Mood::Yes => vec![(&s.yes[..], YES_SIZE, YES)],
        Mood::No => vec![(&s.no[..], 1.0, NO)],
    }
}

/// What Bit looks like at one moment: changing from `before` to `mood`, `u`
/// of the way (1 = done), at animation time `t`.
#[derive(Clone, Copy)]
pub struct Pose {
    pub t: f32,
    pub before: Mood,
    pub mood: Mood,
    pub u: f32,
}

/// Reusable sample buffers, so drawing a frame allocates nothing.
#[derive(Default)]
pub struct Canvas {
    depth: Vec<f32>,
    /// Facet color per sample, `0` where nothing was drawn.
    color: Vec<u32>,
    out: Vec<u32>,
    /// Supersampled width, and the samples drawn last frame (x0, y0, x1, y1):
    /// only those need clearing.
    sw: usize,
    dirty: [usize; 4],
}

impl Canvas {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw Bit with radius `r` (pixels) centered at `(cx, cy)` in a
    /// `side` x `side` bitmap, top row first.
    pub fn render(&mut self, side: usize, cx: f32, cy: f32, r: f32, pose: &Pose) -> &[u32] {
        // Supersampling: 2x2 (like 4x MSAA), 3x3 for small icons.
        let ss = if side <= 128 { 3 } else { 2 };
        let sw = side * ss;
        if sw != self.sw {
            self.depth.clear();
            self.depth.resize(sw * sw, f32::NEG_INFINITY);
            self.color.clear();
            self.color.resize(sw * sw, 0);
            self.sw = sw;
        } else {
            let [x0, y0, x1, y1] = self.dirty;
            for y in y0..y1 {
                self.depth[y * sw + x0..y * sw + x1].fill(f32::NEG_INFINITY);
                self.color[y * sw + x0..y * sw + x1].fill(0);
            }
        }
        let ssf = ss as f32;
        let mut target = Target {
            depth: &mut self.depth,
            color: &mut self.color,
            w: sw,
            drawn: [sw, sw, 0, 0],
        };
        // The eye sits 1000 px in front of Bit.
        draw_morph(
            &mut target,
            pose,
            [cx * ssf, cy * ssf],
            r * ssf,
            1000.0 * ssf,
        );

        let drawn = target.drawn;
        self.dirty = if drawn[0] < drawn[2] { drawn } else { [0; 4] };
        self.out.clear();
        self.out.resize(side * side, 0);
        let n = (ss * ss) as u32;
        let [x0, y0, x1, y1] = self.dirty;
        for oy in y0 / ss..y1.div_ceil(ss) {
            for ox in x0 / ss..x1.div_ceil(ss) {
                let (mut a, mut rr, mut gg, mut bb) = (0u32, 0u32, 0u32, 0u32);
                for sy in 0..ss {
                    let row = (oy * ss + sy) * sw + ox * ss;
                    for &c in &self.color[row..row + ss] {
                        if c != 0 {
                            a += 255;
                            rr += (c >> 16) & 255;
                            gg += (c >> 8) & 255;
                            bb += c & 255;
                        }
                    }
                }
                if a != 0 {
                    self.out[oy * side + ox] =
                        (a / n) << 24 | (rr / n) << 16 | (gg / n) << 8 | (bb / n);
                }
            }
        }
        &self.out
    }
}

struct Target<'a> {
    depth: &'a mut [f32],
    color: &'a mut [u32],
    w: usize,
    /// Bounds of everything rasterized (x0, y0, x1, y1).
    drawn: [usize; 4],
}

/// A change of shape: the new shape swells out from tucked-away
/// while the old one sinks inside it.
fn draw_morph(target: &mut Target, pose: &Pose, center: [f32; 2], r: f32, eye_z: f32) {
    let t = pose.t;
    let e = smoothstep(0.0, 1.0, pose.u);
    let pulse = 1.0 + PULSE * (t * 9.0).sin();
    let rot = rotation(t);
    let mut draw = |mood: Mood, k: f32| {
        for (tris, size, base) in parts(mood, t) {
            draw_shape(
                target,
                tris,
                &rot,
                r * pulse * size * k,
                center,
                eye_z,
                base,
            );
        }
    };
    draw(pose.mood, TUCKED + (1.0 - TUCKED) * e);
    if pose.u < 1.0 && pose.before != pose.mood {
        draw(pose.before, 1.0 - (1.0 - TUCKED) * e);
    }
}

/// Tumbling as it flies: `rotation_y(1.3 t) * rotation_x(0.7 t)`.
fn rotation(t: f32) -> [V3; 3] {
    let (sy, cy) = (t * 1.3).sin_cos();
    let (sx, cx) = (t * 0.7).sin_cos();
    [
        [cy, sy * sx, sy * cx],
        [0.0, cx, -sx],
        [-sy, cy * sx, cy * cx],
    ]
}

/// Each flat facet lit by a fixed light as seen
/// from the front, with a sharp white glint on facets that catch it.
fn draw_shape(
    target: &mut Target,
    tris: &[Tri],
    rot: &[V3; 3],
    scale: f32,
    center: [f32; 2],
    eye_z: f32,
    base: V3,
) {
    let light = norm([0.3, 0.8, 0.5]);
    let eye = [0.0, 0.0, eye_z];
    for (i, f) in tris.iter().enumerate() {
        let p = f.map(|v| {
            let m = |row: V3| (row[0] * v[0] + row[1] * v[1] + row[2] * v[2]) * scale;
            [m(rot[0]), m(rot[1]), m(rot[2])]
        });
        let n = norm(cross(sub(p[1], p[0]), sub(p[2], p[0])));
        // Facing away: hidden behind the front of a closed shape.
        if n[2] <= 0.0 {
            continue;
        }
        let mid = [
            (p[0][0] + p[1][0] + p[2][0]) / 3.0,
            (p[0][1] + p[1][1] + p[2][1]) / 3.0,
            (p[0][2] + p[1][2] + p[2][2]) / 3.0,
        ];
        let view = norm(sub(eye, mid));
        let diffuse = dot(n, light).max(0.0) + if i % 2 == 0 { 0.05 } else { -0.03 };
        let glint = dot(n, norm(add(light, view))).max(0.0).powf(28.0);
        let k = 0.45 + 0.75 * diffuse.clamp(0.0, 1.0);
        let g = (glint * 0.85).min(0.85);
        let rgb = base.map(|c| {
            let lit = (c * k).min(1.0);
            ((lit + (1.0 - lit) * g) * 255.0 + 0.5) as u32
        });
        let color = 0xff00_0000 | rgb[0] << 16 | rgb[1] << 8 | rgb[2];
        let screen = p.map(|v| [center[0] + v[0], center[1] - v[1], v[2]]);
        raster(target, screen, color);
    }
}

/// Fill a triangle into the sample buffers, keeping the nearest surface.
fn raster(target: &mut Target, p: [V3; 3], color: u32) {
    let edge =
        |a: V3, b: V3, x: f32, y: f32| (x - a[0]) * (b[1] - a[1]) - (y - a[1]) * (b[0] - a[0]);
    let area = edge(p[0], p[1], p[2][0], p[2][1]);
    if area.abs() < 1e-6 {
        return;
    }
    let inv = 1.0 / area;
    let w = target.w as f32;
    let lo = |k: usize| p.iter().map(|v| v[k]).fold(f32::INFINITY, f32::min);
    let hi = |k: usize| p.iter().map(|v| v[k]).fold(f32::NEG_INFINITY, f32::max);
    let x0 = (lo(0) - 0.5).floor().max(0.0) as usize;
    let y0 = (lo(1) - 0.5).floor().max(0.0) as usize;
    let x1 = (hi(0) + 0.5).ceil().min(w) as usize;
    let y1 = (hi(1) + 0.5).ceil().min(w) as usize;
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let d = &mut target.drawn;
    *d = [d[0].min(x0), d[1].min(y0), d[2].max(x1), d[3].max(y1)];
    // A hair of overlap so neighbouring facets never leave pinholes.
    const EPS: f32 = -1e-4;
    // Barycentric weights and depth step linearly along a row.
    let (dw0, dw1) = ((p[2][1] - p[1][1]) * inv, (p[0][1] - p[2][1]) * inv);
    let (dz0, dz1) = (p[0][2] - p[2][2], p[1][2] - p[2][2]);
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        let px = x0 as f32 + 0.5;
        let mut w0 = edge(p[1], p[2], px, py) * inv;
        let mut w1 = edge(p[2], p[0], px, py) * inv;
        let row = &mut target.depth[y * target.w + x0..y * target.w + x1];
        let colors = &mut target.color[y * target.w + x0..y * target.w + x1];
        for (d, c) in row.iter_mut().zip(colors.iter_mut()) {
            if w0 >= EPS && w1 >= EPS && 1.0 - w0 - w1 >= EPS {
                let z = p[2][2] + w0 * dz0 + w1 * dz1;
                if z > *d {
                    *d = z;
                    *c = color;
                }
            }
            w0 += dw0;
            w1 += dw1;
        }
    }
}

/// Bit at rest as a small straight-alpha RGBA image (for the tray and window icons).
pub fn icon_rgba(side: usize) -> Vec<u8> {
    let mut canvas = Canvas::new();
    let c = side as f32 * 0.5;
    // The 168-facet neutral form fully out (the other tucked inside it), at
    // the angle chosen for the icon.
    let pose = Pose { t: 0.78125, before: Mood::Neutral, mood: Mood::Neutral, u: 1.0 };
    // Sized to the neutral ball (radius 1, plus its pulse) so it fills the icon.
    let px = canvas.render(side, c, c, c * 0.92 / (1.0 + PULSE), &pose);
    let mut rgba = Vec::with_capacity(side * side * 4);
    for &p in px {
        let a = p >> 24;
        let un = |v: u32| {
            if a == 0 {
                0
            } else {
                ((v * 255 + a / 2) / a).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[
            un((p >> 16) & 255),
            un((p >> 8) & 255),
            un(p & 255),
            a as u8,
        ]);
    }
    rgba
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    let l = len(a);
    if l > 0.0 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes Bit's moods over a dark background to $BIT_SNAPSHOT (a .ppm)
    /// for eyeballing.
    #[test]
    fn snapshot() {
        let Some(path) = std::env::var_os("BIT_SNAPSHOT") else {
            return;
        };
        let side = 200;
        let poses = [
            Pose {
                t: 0.4,
                before: Mood::Neutral,
                mood: Mood::Neutral,
                u: 1.0,
            },
            Pose {
                t: 1.9,
                before: Mood::Neutral,
                mood: Mood::Neutral,
                u: 1.0,
            },
            Pose {
                t: 2.3,
                before: Mood::Neutral,
                mood: Mood::Yes,
                u: 0.5,
            },
            Pose {
                t: 3.1,
                before: Mood::Neutral,
                mood: Mood::Yes,
                u: 1.0,
            },
            Pose {
                t: 4.0,
                before: Mood::Yes,
                mood: Mood::No,
                u: 0.5,
            },
            Pose {
                t: 5.2,
                before: Mood::Neutral,
                mood: Mood::No,
                u: 1.0,
            },
        ];
        let mut img = vec![[12u8, 18, 24]; side * side * poses.len()];
        let mut canvas = Canvas::new();
        for (k, pose) in poses.iter().enumerate() {
            let px = canvas.render(side, 100.3, 100.7, 60.0, pose);
            for y in 0..side {
                for x in 0..side {
                    let p = px[y * side + x];
                    let a = (p >> 24) as u32;
                    let o = &mut img[y * side * poses.len() + k * side + x];
                    let ch = [(p >> 16) & 255, (p >> 8) & 255, p & 255];
                    for i in 0..3 {
                        o[i] = (ch[i] + o[i] as u32 * (255 - a) / 255).min(255) as u8;
                    }
                }
            }
        }
        let mut out = format!("P6 {} {} 255\n", side * poses.len(), side).into_bytes();
        out.extend(img.iter().flatten());
        std::fs::write(path, out).unwrap();
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    fn render_speed() {
        if std::env::var_os("BIT_BENCH").is_none() {
            return;
        }
        let mut canvas = Canvas::new();
        for side in [100usize, 190, 320, 600] {
            let start = std::time::Instant::now();
            for i in 0..200 {
                let pose = Pose {
                    t: i as f32 * 0.016,
                    before: Mood::Neutral,
                    mood: Mood::Neutral,
                    u: 1.0,
                };
                let c = side as f32 * 0.5;
                canvas.render(side, c, c, c / reach(), &pose);
            }
            eprintln!("side {side}: {:?} per frame", start.elapsed() / 200);
        }
    }
}

#[cfg(test)]
mod frames {
    use super::*;

    /// Writes an animation of Bit (neutral, yes, neutral, no) as raw
    /// straight-alpha RGBA frames to $BIT_FRAMES, for the README.
    #[test]
    fn frames() {
        let Some(path) = std::env::var_os("BIT_FRAMES") else {
            return;
        };
        const STEP: f32 = 1.4;
        const MOODS: [Mood; 4] = [Mood::Neutral, Mood::Yes, Mood::Neutral, Mood::No];
        let side = 240;
        let mut canvas = Canvas::new();
        let mut out = Vec::new();
        for i in 0..(4.0 * STEP * 30.0) as usize {
            let t = i as f32 / 30.0;
            let k = (t / STEP) as usize;
            let u = ((t - k as f32 * STEP) / MORPH).min(1.0);
            let pose = Pose {
                t,
                before: MOODS[(k + 3) % 4],
                mood: MOODS[k % 4],
                u,
            };
            let c = side as f32 * 0.5;
            for &p in canvas.render(side, c, c, c * 0.92 / reach(), &pose) {
                let a = p >> 24;
                let un = |v: u32| {
                    if a == 0 {
                        0
                    } else {
                        ((v * 255 + a / 2) / a).min(255) as u8
                    }
                };
                out.extend_from_slice(&[
                    un((p >> 16) & 255),
                    un((p >> 8) & 255),
                    un(p & 255),
                    a as u8,
                ]);
            }
        }
        std::fs::write(path, out).unwrap();
    }
}

#[cfg(test)]
mod icon_file {
    use super::*;

    /// Writes Bit's icon at the sizes Windows uses as raw RGBA files in
    /// $BIT_ICONS (a folder), to rebuild assets/bit.ico from.
    #[test]
    fn icons() {
        let Some(dir) = std::env::var_os("BIT_ICONS") else { return };
        for side in [16usize, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024] {
            std::fs::write(std::path::Path::new(&dir).join(format!("{side}.rgba")), icon_rgba(side)).unwrap();
        }
    }
}
