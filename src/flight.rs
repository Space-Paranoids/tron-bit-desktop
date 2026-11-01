//! Where Bit is during a visit, and what shape it is in. Bit darts in from
//! off screen (sometimes from across the far side, sometimes past the viewer, big and close), brakes to a stop, drifts about one spot,
//! maybe says yes or no (changing its mind on a longer visit), then speeds
//! off, faster and faster. Everything is a function of the visit's clock, so
//! motion, shape and voice can never drift apart.

use crate::bit::{self, Mood, MORPH};
use crate::settings::Settings;

/// One display, in physical desktop pixels, with its scale (1.0 at 96 dpi).
#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub scale: f32,
    /// The main display.
    pub primary: bool,
}

impl Monitor {
    /// How a screen is remembered in settings: its place and size on the
    /// desktop, which stay the same across restarts until the screens are
    /// rearranged.
    pub fn key(&self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.w, self.h)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voice {
    Yes,
    No,
    NoNoNo,
    YesYesYes,
}

/// The Screens setting.
pub const SCREENS_MAIN: u8 = 1;
pub const SCREENS_NOT_MAIN: u8 = 2;
pub const SCREENS_CHOSEN: u8 = 3;
/// The "Hangs out" setting.
pub const SPOT_CENTER: u8 = 1;
pub const SPOT_TOP: u8 = 2;
pub const SPOT_BOTTOM: u8 = 3;
pub const SPOT_LEFT: u8 = 4;
pub const SPOT_RIGHT: u8 = 5;
pub const SPOT_CORNERS: u8 = 6;

/// How many times its usual size Bit is when it passes close to the viewer.
const CLOSE: f32 = 3.5;
/// How long Bit takes to shoot off when clicked (s).
const SHOO: f32 = 0.6;
/// Length of the "No, no, no..." recording (s).
const NONONO_SECONDS: f32 = 1.43;
/// Length of the "Yes, yes, yes..." recording (s).
const YESYESYES_SECONDS: f32 = 1.35;

/// How long Bit holds a long answer.
fn long_answer(mood: Mood) -> f32 {
    if mood == Mood::Yes {
        YESYESYES_SECONDS
    } else {
        NONONO_SECONDS
    }
}

/// A small xorshift generator: plenty for picking flight paths.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Rng((nanos ^ (u64::from(std::process::id()) << 32)) | 1)
    }

    /// A generator of its own, for a flight that runs for a long time.
    pub fn split(&mut self) -> Rng {
        self.next();
        Rng((self.0 ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// A number in 0..1.
    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / 16_777_216.0
    }
}

/// Bit at one moment of a visit.
#[derive(Clone, Copy)]
pub struct Frame {
    pub pos: [f32; 2],
    /// Radius in pixels.
    pub r: f32,
    pub before: Mood,
    pub mood: Mood,
    pub u: f32,
}

struct Shoo {
    since: f32,
    from: [f32; 2],
    to: [f32; 2],
    r: f32,
    /// Size it ends at (it shrinks away when no clear way off screen).
    end: f32,
    before: Mood,
}

pub struct Visit {
    monitors: Vec<Monitor>,
    /// Usual radius (px) on the chosen screen.
    r: f32,
    stay: f32,
    from: [f32; 2],
    from_size: f32,
    hover: [f32; 2],
    to: [f32; 2],
    to_size: f32,
    /// Seconds to arrive, and when it starts to leave.
    arrive: f32,
    leave: f32,
    /// How far it drifts about its spot (px).
    drift: f32,
    said: Option<Mood>,
    /// Says yes or no once, rather than changing its mind.
    single: bool,
    /// Says the long "yes, yes, yes" or "no, no, no".
    triple: bool,
    /// Seconds it hovers before speaking.
    think: f32,
    shoo: Option<Shoo>,
    /// Screens it must not be seen on (from the Screens setting).
    avoid: Vec<Monitor>,
    /// Answering a question asked by clicking it.
    asked: bool,
    /// The screens it may be on.
    allowed: Vec<Monitor>,
    /// Being held by the pointer, here.
    held: Option<[f32; 2]>,
}

impl Visit {
    /// Plan a visit to one of `monitors`. `forced` is a voice asked for from
    /// the settings window; otherwise Bit decides for itself. `ask` is the
    /// magic-eight-ball visit: in quickly, a moment's thought, the answer,
    /// and off again.
    pub fn new(
        rng: &mut Rng,
        monitors: Vec<Monitor>,
        s: &Settings,
        forced: Option<Voice>,
        ask: bool,
    ) -> Visit {
        let mut d = || rng.next();
        // The screens it may visit, and the ones it keeps off entirely.
        let (allowed, avoid): (Vec<Monitor>, Vec<Monitor>) =
            monitors.iter().partition(|m| match s.screens {
                SCREENS_MAIN => m.primary,
                SCREENS_NOT_MAIN => !m.primary,
                SCREENS_CHOSEN => s.chosen_screens.contains(&m.key()),
                _ => true,
            });
        let (allowed, avoid) = if allowed.is_empty() {
            (monitors.clone(), Vec::new())
        } else {
            (allowed, avoid)
        };
        let m = allowed[((d() * allowed.len() as f32) as usize).min(allowed.len() - 1)];
        let on_screens = allowed.clone();
        let r = s.size_pixels * 0.5 * m.scale;
        let reach = bit::reach();

        let quick = forced.is_none() && d() < 0.25;
        let said = match forced {
            Some(Voice::Yes | Voice::YesYesYes) => Some(Mood::Yes),
            Some(_) => Some(Mood::No),
            None if d() < f32::from(s.chattiness) / 100.0 => {
                Some(if d() < 0.55 { Mood::Yes } else { Mood::No })
            }
            None => None,
        };
        let wants_long = match said {
            Some(Mood::Yes) => s.yes3_sound_enabled,
            Some(Mood::No) => s.alarm_sound_enabled,
            _ => false,
        };
        let triple = matches!(forced, Some(Voice::NoNoNo | Voice::YesYesYes))
            || (forced.is_none() && !quick && wants_long && d() < 0.2);
        let single = quick || triple || forced.is_some();

        // It hovers somewhere on the chosen screen, and comes in from the
        // side, from the far side (flying right across), from above or
        // below, or past the viewer (big and close, shrinking into place).
        // Kept far enough in that Bit, drifting, stays on this screen.
        let hover = spot(m, s.spot, r * (reach + 0.8) + 4.0, &mut d);
        let left = hover[0] < m.x + m.w * 0.5;
        let side_x = |near: bool| if near == left { m.x - r } else { m.x + m.w + r };
        let far_y = |d: f32, e: f32| match e {
            e if e < 0.5 => m.y + m.h * (0.15 + 0.7 * d),
            e if e < 0.75 => m.y - 0.15 * m.h,
            _ => m.y + 1.15 * m.h,
        };
        let edge_y = |below: bool| if below { m.y + m.h + r } else { m.y - r };
        // Start and end where no screen can see it, which on a multi-screen
        // desktop can mean flying in across a neighbouring screen, but never
        // across a screen it's kept off. If no way in (or out) keeps clear of
        // those, it comes from (or goes into) the distance instead.
        let path = |enter: bool, d: &mut dyn FnMut() -> f32| {
            for _ in 0..16 {
                let pick = d();
                let (p, size) = if enter {
                    match pick {
                        e if e < 0.45 => ([side_x(false), far_y(d(), d())], 1.0),
                        e if e < 0.65 => ([side_x(true), hover[1] + (d() - 0.5) * 0.4 * m.h], 1.0),
                        e if e < 0.85 => {
                            ([hover[0] + (d() - 0.5) * 0.3 * m.w, edge_y(d() < 0.5)], 1.0)
                        }
                        _ => ([m.x + m.w * (0.3 + 0.4 * d()), edge_y(d() < 0.5)], CLOSE),
                    }
                } else {
                    match pick {
                        e if e < 0.45 => ([side_x(false), far_y(d(), d())], 1.0),
                        e if e < 0.6 => ([hover[0], edge_y(false)], 1.0),
                        e if e < 0.75 => ([hover[0], edge_y(true)], 1.0),
                        e if e < 0.85 => ([side_x(true), hover[1] - 0.1 * m.h], 1.0),
                        _ => ([m.x + m.w * (0.3 + 0.4 * d()), edge_y(d() < 0.5)], CLOSE),
                    }
                };
                let p = out_of_sight(&monitors, p, hover, r * size * reach + 4.0);
                if clear(&avoid, p, size, hover, 1.0, r * reach) {
                    return (p, size);
                }
            }
            (hover, 0.0)
        };
        let (from, from_size) = path(true, &mut d);
        let (to, to_size) = path(false, &mut d);

        // Seconds to fly in and out, a little longer for
        // a crossing or a long trip over other screens.
        let trip = |p: [f32; 2]| (dist(p, hover) / m.w.max(m.h) - 0.6).max(0.0) * 0.35;
        let arrive = if quick {
            0.9
        } else if ask {
            1.2
        } else {
            1.6
        } + trip(from);
        let leave_for = if quick {
            0.75
        } else if ask {
            1.2
        } else {
            1.45
        } + trip(to);
        let think = if ask {
            1.2
        } else if quick {
            0.25
        } else {
            0.5
        };
        let talking = if ask {
            2.0
        } else if triple {
            long_answer(said.unwrap_or(Mood::No)) + 0.6
        } else if single {
            1.2
        } else {
            2.4
        };
        let base = if quick || ask {
            2.4 + d() * 1.2
        } else {
            s.visible_seconds as f32 * (0.85 + 0.3 * d())
        };
        let stay = base.max(arrive + think + talking + 0.4 + leave_for);

        Visit {
            monitors,
            r,
            stay,
            from,
            from_size,
            hover,
            to,
            to_size,
            arrive,
            leave: stay - leave_for,
            drift: r * 0.4,
            said,
            single,
            triple,
            think,
            shoo: None,
            avoid,
            asked: false,
            allowed: on_screens,
            held: None,
        }
    }

    /// The voice for a change to `mood`.
    pub fn voice(&self, mood: Mood) -> Option<Voice> {
        match mood {
            Mood::Yes if self.triple => Some(Voice::YesYesYes),
            Mood::Yes => Some(Voice::Yes),
            Mood::No if self.triple => Some(Voice::NoNoNo),
            Mood::No => Some(Voice::No),
            Mood::Neutral => None,
        }
    }

    /// Bit `t` seconds into the visit, or `None` once it has gone.
    pub fn at(&self, t: f32) -> Option<Frame> {
        if let Some(s) = &self.shoo {
            let k = (t - s.since) / SHOO;
            if k >= 1.0 {
                return None;
            }
            let u = ((t - s.since) / MORPH).min(1.0);
            return Some(Frame {
                pos: lerp(s.from, s.to, k * k * k),
                r: s.r * (1.0 + (s.end - 1.0) * k * k),
                before: s.before,
                mood: Mood::Neutral,
                u,
            });
        }
        if t >= self.stay {
            return None;
        }
        let drift = |th: f32| {
            [
                self.hover[0] + (1.8 * th).sin() * self.drift,
                self.hover[1] + ((1.4 * th + 1.0).sin() - 1f32.sin()) * self.drift,
            ]
        };
        let (pos, size) = if t < self.arrive {
            // Fast in, braking to a stop.
            let u = 1.0 - (1.0 - t / self.arrive).powi(3);
            (
                lerp(self.from, self.hover, u),
                self.from_size + (1.0 - self.from_size) * u.sqrt(),
            )
        } else if t < self.leave {
            // Drifting about the spot.
            (drift(t - self.arrive), 1.0)
        } else {
            // Speeding off, faster and faster.
            let u = (t - self.leave) / (self.stay - self.leave);
            (
                lerp(drift(self.leave - self.arrive), self.to, u * u * u),
                1.0 + (self.to_size - 1.0) * u * u,
            )
        };

        // While it hovers it may say something, morphing into the answer and
        // back to neutral; on a longer visit it changes its mind first.
        let other = |m: Mood| if m == Mood::Yes { Mood::No } else { Mood::Yes };
        let talk_from = self.arrive + self.think;
        let talk_to = self.leave - 0.4;
        let mid = if self.single {
            talk_to
        } else {
            (talk_from + talk_to) * 0.5
        };
        let morph = |since: f32| (since / MORPH).min(1.0);
        let (before, mood, u) = match self.said {
            Some(m) if t > talk_from && t < mid => (Mood::Neutral, m, morph(t - talk_from)),
            Some(m) if t >= mid && t < talk_to => (m, other(m), morph(t - mid)),
            Some(m) if t >= talk_to => (
                if self.single { m } else { other(m) },
                Mood::Neutral,
                morph(t - talk_to),
            ),
            _ => (Mood::Neutral, Mood::Neutral, 1.0),
        };
        Some(Frame {
            pos: self.held.unwrap_or(pos),
            r: self.r * size,
            before,
            mood,
            u,
        })
    }

    /// Whether it can be picked up (not while it's shooting off).
    pub fn grabbable(&self) -> bool {
        self.shoo.is_none()
    }

    /// Held by the pointer at `pos`, kept wholly on the screens it may visit.
    pub fn hold(&mut self, pos: Option<[f32; 2]>) {
        let rad = self.r * bit::reach() + 4.0;
        self.held = pos.map(|p| keep_on(&self.allowed, p, rad));
    }

    /// Let go at `pos`, `t` seconds in: it hovers there and carries on.
    pub fn place(&mut self, t: f32, pos: [f32; 2]) {
        let rad = self.r * bit::reach() + 4.0;
        let pos = keep_on(&self.allowed, pos, rad);
        self.held = None;
        if t < self.arrive {
            self.arrive = t;
        }
        if t >= self.leave {
            // Dropped on its way out: it settles a moment before leaving.
            let leave_for = self.stay - self.leave;
            self.leave = t + 0.8;
            self.stay = self.leave + leave_for;
        }
        let th = t - self.arrive;
        self.hover = [
            pos[0] - (1.8 * th).sin() * self.drift,
            pos[1] - ((1.4 * th + 1.0).sin() - 1f32.sin()) * self.drift,
        ];
        // Its way out may now cross a screen it's kept off.
        let big = self.to_size.max(1.0);
        if !clear(&self.avoid, self.to, big, self.hover, big, self.r * bit::reach()) {
            self.to = self.hover;
            self.to_size = 0.0;
        }
    }

    /// Clicked to shoo: Bit shoots off the nearer side of its screen (or
    /// another side, if that one would cross a screen it's kept off).
    pub fn shoo(&mut self, t: f32) {
        if self.shoo.is_some() {
            return;
        }
        let Some(f) = self.at(t) else { return };
        let m = self
            .monitors
            .iter()
            .copied()
            .min_by(|a, b| dist(centre(a), f.pos).total_cmp(&dist(centre(b), f.pos)))
            .unwrap();
        let near = if f.pos[0] < m.x + m.w * 0.5 {
            -1.0
        } else {
            1.0
        };
        let rad = f.r * bit::reach();
        let ways = [
            [near * 0.7, -0.15],
            [-near * 0.7, -0.15],
            [0.0, -0.8],
            [0.0, 0.8],
        ];
        let (to, end) = ways
            .iter()
            .map(|w| {
                let to = [f.pos[0] + w[0] * m.w, f.pos[1] + w[1] * m.h];
                out_of_sight(&self.monitors, to, f.pos, rad + 4.0)
            })
            .find(|&to| clear(&self.avoid, to, 1.0, f.pos, 1.0, rad))
            .map_or((f.pos, 0.0), |to| (to, 1.0));
        self.shoo = Some(Shoo {
            since: t,
            from: f.pos,
            to,
            r: f.r,
            end,
            before: f.mood,
        });
    }

    /// Clicked to ask: Bit stops where it is, thinks for a moment and says
    /// `mood`, then carries on its way. False if it's already leaving or
    /// answering.
    pub fn answer(&mut self, t: f32, mood: Mood) -> bool {
        if self.shoo.is_some() || self.asked || t >= self.leave - 0.4 {
            return false;
        }
        let leave_for = self.stay - self.leave;
        let start = t.max(self.arrive) + 0.35;
        self.said = Some(mood);
        self.single = true;
        self.triple = false;
        self.think = start - self.arrive;
        self.leave = self.leave.max(start + 2.4);
        self.stay = self.leave + leave_for;
        self.asked = true;
        true
    }
}

/// Answering a question while always on.
struct Talk {
    /// When it starts to change shape (after a moment's thought).
    start: f32,
    mood: Mood,
    /// How long it holds the answer.
    hold: f32,
    triple: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Pace {
    /// Wandering about at an easy speed.
    Cruise,
    /// A sudden dash, often straight at a wall.
    Lunge,
    /// Slowing to a near stop, as if thinking about the next try.
    Stall,
}

/// The physics step (s): small and fixed, so motion is the same at any
/// frame rate.
const STEP: f32 = 1.0 / 240.0;
/// How far Bit bobs about its path while roaming (times its radius).
const WOBBLE: f32 = 0.08;
/// How far Bit drifts about its spot while hovering (times its radius).
const HOVER_DRIFT: f32 = 0.3;
/// The largest it breathes out to while roaming (times its usual size).
const BREATH: f32 = 1.06;

/// Always on: Bit roams one screen non-stop, like something trapped behind
/// the glass. It ricochets off the edges, wanders, lunges at random (often
/// at a wall, testing it for a way out), stalls, and sets off again. It flies
/// in from off screen to start and speeds off the nearest side to stop.
pub struct Roam {
    monitors: Vec<Monitor>,
    m: Monitor,
    rng: Rng,
    /// Usual radius (px).
    r: f32,
    /// 0..1, from the Restlessness setting.
    rest: f32,
    pos: [f32; 2],
    vel: [f32; 2],
    /// The way it wants to go (radians), and how fast (px/s).
    heading: f32,
    speed: f32,
    /// Slow wandering of the heading while cruising (radians/s).
    turn: f32,
    pace: Pace,
    until: f32,
    /// Time simulated so far.
    clock: f32,
    /// Fully on its screen (the edges hold it from then on).
    inside: bool,
    leaving: bool,
    talk: Option<Talk>,
    /// Being held by the pointer, here.
    held: Option<[f32; 2]>,
    /// How far Bit reaches now (times its radius): the neutral shape's
    /// reach, easing out to the spiky answer's while it answers, so it can
    /// roam right up to the edges and still keep its answer on screen.
    extent: f32,
    /// Hovering about this spot instead of roaming, and the Hangs out
    /// choice it came from.
    hover: Option<[f32; 2]>,
    spot: u8,
}

impl Roam {
    pub fn new(mut rng: Rng, monitors: Vec<Monitor>, s: &Settings) -> Roam {
        let m = monitors
            .iter()
            .copied()
            .find(|m| m.key() == s.always_screen)
            .or_else(|| monitors.iter().copied().find(|m| m.primary))
            .unwrap_or(monitors[0]);
        let r = s.size_pixels * 0.5 * m.scale;
        let rad = r * bit::reach() * BREATH + 4.0;
        // Come in over whichever side has the shortest way in from off
        // every screen (a side with no neighbouring screen, if there is one).
        let c = centre(&m);
        let mut best: Option<([f32; 2], f32)> = None;
        for _ in 0..8 {
            let along = 0.2 + 0.6 * rng.next();
            let p = match (rng.next() * 4.0) as u32 {
                0 => [m.x - rad, m.y + m.h * along],
                1 => [m.x + m.w + rad, m.y + m.h * along],
                2 => [m.x + m.w * along, m.y - rad],
                _ => [m.x + m.w * along, m.y + m.h + rad],
            };
            let p = out_of_sight(&monitors, p, c, rad);
            let d = dist(p, c);
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((p, d));
            }
        }
        let pos = best.map_or([m.x - rad, c[1]], |(p, _)| p);
        let aim = [
            m.x + m.w * (0.3 + 0.4 * rng.next()),
            m.y + m.h * (0.3 + 0.4 * rng.next()),
        ];
        let heading = (aim[1] - pos[1]).atan2(aim[0] - pos[0]);
        let speed = r * 9.0;
        let mut roam = Roam {
            monitors,
            m,
            rng,
            r,
            rest: f32::from(s.restlessness.min(100)) / 100.0,
            pos,
            vel: [heading.cos() * speed, heading.sin() * speed],
            heading,
            speed,
            turn: 0.0,
            pace: Pace::Lunge,
            until: f32::MAX,
            clock: 0.0,
            inside: false,
            leaving: false,
            talk: None,
            held: None,
            extent: bit::neutral_reach(),
            hover: None,
            spot: s.spot,
        };
        roam.hover = s.always_hover.then(|| roam.hover_spot());
        roam
    }

    /// Where to hover, for the Hangs out setting: dead centre for Center,
    /// and always in the chosen part of the screen.
    fn hover_spot(&mut self) -> [f32; 2] {
        if self.spot == SPOT_CENTER {
            return centre(&self.m);
        }
        let margin = self.r * (bit::reach() + HOVER_DRIFT) + 4.0;
        let (m, choice) = (self.m, self.spot);
        // (`spot` wanders off now and then if its first number is high.)
        let mut first = true;
        let rng = &mut self.rng;
        spot(m, choice, margin, &mut || {
            if std::mem::take(&mut first) {
                0.0
            } else {
                rng.next()
            }
        })
    }

    /// Settings changed: size (px across, as in Settings) and restlessness.
    pub fn update(&mut self, s: &Settings) {
        self.r = s.size_pixels * 0.5 * self.m.scale;
        self.rest = f32::from(s.restlessness.min(100)) / 100.0;
        if s.always_hover != self.hover.is_some() || s.spot != self.spot {
            self.spot = s.spot;
            self.hover = s.always_hover.then(|| self.hover_spot());
            if self.hover.is_none() {
                // Set off roaming.
                self.until = self.clock;
            }
        }
    }

    /// The spot it hovers about.
    #[cfg(test)]
    pub fn hovering(&self) -> Option<[f32; 2]> {
        self.hover
    }

    /// The screen it roams.
    #[cfg(test)]
    pub fn screen(&self) -> Monitor {
        self.m
    }

    /// Whether it can be picked up (not while flying in or leaving).
    pub fn grabbable(&self) -> bool {
        self.inside && !self.leaving
    }

    /// Held by the pointer at `pos`, kept on its screen.
    pub fn hold(&mut self, pos: Option<[f32; 2]>) {
        let rad = self.r * bit::reach() + 4.0;
        self.held = pos.map(|p| keep_on(&[self.m], p, rad));
    }

    /// Let go at `pos`: roaming, it sets off from there; hovering, that's
    /// its new spot.
    pub fn place(&mut self, pos: [f32; 2]) {
        let rad = self.r * bit::reach() + 4.0;
        let pos = keep_on(&[self.m], pos, rad);
        self.held = None;
        self.pos = pos;
        self.vel = [0.0, 0.0];
        self.pace = Pace::Stall;
        self.until = self.clock + 0.3;
        if self.hover.is_some() {
            self.hover = Some(pos);
        }
    }

    /// Speed off the nearest side, and be gone.
    pub fn leave(&mut self) {
        if self.leaving {
            return;
        }
        self.leaving = true;
        if let Some(p) = self.held.take() {
            self.pos = p;
        }
        let rad = self.r * bit::reach() * BREATH + 4.0;
        let (m, p) = (self.m, self.pos);
        let to = [
            [m.x - rad, p[1]],
            [m.x + m.w + rad, p[1]],
            [p[0], m.y - rad],
            [p[0], m.y + m.h + rad],
        ]
        .into_iter()
        .map(|w| out_of_sight(&self.monitors, w, p, rad))
        .min_by(|a, b| dist(*a, p).total_cmp(&dist(*b, p)))
        .unwrap();
        self.heading = (to[1] - p[1]).atan2(to[0] - p[0]);
    }

    pub fn leaving(&self) -> bool {
        self.leaving
    }

    /// Startled (clicked, with Clicking Bit set to shoo): a dash in a new
    /// direction.
    pub fn startle(&mut self, t: f32) {
        if self.leaving || !self.inside || self.talking(t) {
            return;
        }
        self.heading = self.rng.next() * std::f32::consts::TAU;
        self.pace = Pace::Lunge;
        self.speed = self.lunge_speed() * 1.3;
        self.until = t + 0.5;
    }

    /// Asked a question: it stops, thinks a moment and says `mood` (with
    /// `triple`, the long "yes, yes, yes" or "no, no, no"). False if it's already answering
    /// or leaving.
    pub fn answer(&mut self, t: f32, mood: Mood, triple: bool) -> bool {
        if self.leaving || self.talking(t) {
            return false;
        }
        let hold = if triple { long_answer(mood) + 0.6 } else { 2.0 };
        let start = t + 0.35;
        self.talk = Some(Talk {
            start,
            mood,
            hold,
            triple,
        });
        self.pace = Pace::Stall;
        self.speed = self.r * 0.15;
        self.until = start + hold + MORPH;
        true
    }

    /// Busy answering, until it's back to its usual shape.
    pub fn talking(&self, t: f32) -> bool {
        self.talk
            .as_ref()
            .is_some_and(|k| t < k.start + k.hold + MORPH)
    }

    /// The voice for a change to `mood`.
    pub fn voice(&self, mood: Mood) -> Option<Voice> {
        let triple = self.talk.as_ref().is_some_and(|k| k.triple);
        match mood {
            Mood::Yes if triple => Some(Voice::YesYesYes),
            Mood::Yes => Some(Voice::Yes),
            Mood::No if triple => Some(Voice::NoNoNo),
            Mood::No => Some(Voice::No),
            Mood::Neutral => None,
        }
    }

    fn cruise_speed(&mut self) -> f32 {
        self.r * (1.2 + 3.0 * self.rest) * (0.7 + 0.6 * self.rng.next())
    }

    fn lunge_speed(&mut self) -> f32 {
        self.r * (7.0 + 9.0 * self.rest) * (0.8 + 0.4 * self.rng.next())
    }

    /// Pick what to do next, at time `t`.
    fn next_pace(&mut self, t: f32) {
        if self.hover.is_some() {
            self.pace = Pace::Stall;
            self.until = f32::MAX;
            return;
        }
        let k = self.rng.next();
        let rest = self.rest;
        let lunge = match self.pace {
            Pace::Cruise => k < 0.25 + 0.5 * rest,
            Pace::Lunge => false,
            Pace::Stall => k < 0.5 + 0.3 * rest,
        };
        let stall = !lunge
            && match self.pace {
                Pace::Cruise => k < 0.6 + 0.25 * rest,
                Pace::Lunge => k < 0.5,
                Pace::Stall => false,
            };
        if lunge {
            self.pace = Pace::Lunge;
            self.until = t + 0.2 + 0.35 * self.rng.next();
            // Usually straight at a wall, as if trying it for a way out.
            self.heading = if self.rng.next() < 0.6 {
                let m = self.m;
                let (a, b) = (self.rng.next(), self.rng.next());
                let wall = match (b * 4.0) as u32 {
                    0 => [m.x, m.y + m.h * a],
                    1 => [m.x + m.w, m.y + m.h * a],
                    2 => [m.x + m.w * a, m.y],
                    _ => [m.x + m.w * a, m.y + m.h],
                };
                (wall[1] - self.pos[1]).atan2(wall[0] - self.pos[0])
            } else {
                self.rng.next() * std::f32::consts::TAU
            };
            self.speed = self.lunge_speed();
        } else if stall {
            self.pace = Pace::Stall;
            self.until = t + (0.4 + 1.8 * self.rng.next()) * (1.3 - 0.8 * rest);
            self.speed = self.r * 0.15;
        } else {
            self.pace = Pace::Cruise;
            self.until = t + (0.8 + 2.2 * self.rng.next()) * (1.4 - rest);
            self.heading += (self.rng.next() - 0.5) * 2.0;
            self.speed = self.cruise_speed();
        }
    }

    /// Run the physics on to time `t`.
    fn advance(&mut self, t: f32) {
        while self.clock + STEP <= t {
            self.clock += STEP;
            let now = self.clock;
            if self.leaving {
                // Faster and faster.
                self.speed = self.speed.max(self.r * 4.0) * (1.0 + 3.0 * STEP);
            } else if self.inside && now >= self.until {
                self.next_pace(now);
            }
            if self.pace == Pace::Cruise && !self.leaving {
                self.turn =
                    (self.turn * 0.995 + (self.rng.next() - 0.5) * 0.08).clamp(-1.2, 1.2);
                self.heading += self.turn * STEP;
            }
            // Ease towards the wanted velocity: snappy for a lunge, gentler
            // otherwise.
            let k = match self.pace {
                _ if self.leaving => 8.0,
                Pace::Lunge => 10.0,
                Pace::Stall => 3.5,
                Pace::Cruise => 2.5,
            };
            let mut want = [
                self.heading.cos() * self.speed,
                self.heading.sin() * self.speed,
            ];
            // Hovering: drawn back to its spot, braking as it gets there,
            // and drifting slowly about it (a dash when startled aside).
            if let Some(h) = self.hover {
                if !self.leaving && (self.pace != Pace::Lunge || !self.inside) {
                    let d = self.r * HOVER_DRIFT;
                    let to = [
                        h[0] + (0.7 * now).sin() * d - self.pos[0],
                        h[1] + (0.53 * now + 1.0).sin() * d - self.pos[1],
                    ];
                    let top = self.r * 9.0;
                    let pull = (2.0_f32).min(top / (to[0].hypot(to[1]) + 1.0));
                    want = [to[0] * pull, to[1] * pull];
                }
            }
            let ease = 1.0 - (-k * STEP).exp();
            for i in 0..2 {
                self.vel[i] += (want[i] - self.vel[i]) * ease;
                self.pos[i] += self.vel[i] * STEP;
            }
            if self.leaving {
                continue;
            }
            let want = if self.talking(now) {
                bit::reach()
            } else {
                bit::neutral_reach()
            };
            self.extent += (want - self.extent) * (1.0 - (-6.0 * STEP).exp());
            let m = self.m;
            let g = (self.r * (self.extent * BREATH + WOBBLE) + 2.0)
                .min(m.w * 0.5)
                .min(m.h * 0.5);
            let (x0, x1, y0, y1) = (m.x + g, m.x + m.w - g, m.y + g, m.y + m.h - g);
            if !self.inside {
                // Flying in: the edges hold it once it's all the way on.
                if (x0..=x1).contains(&self.pos[0]) && (y0..=y1).contains(&self.pos[1]) {
                    self.inside = true;
                    self.pace = Pace::Cruise;
                    self.speed = self.cruise_speed();
                    self.until = now + 0.6 + self.rng.next();
                }
                continue;
            }
            // Ricochet off the edges, a little off true, losing some speed.
            let mut hit = false;
            let jitter = (self.rng.next() - 0.5) * 0.5;
            if self.pos[0] < x0 || self.pos[0] > x1 {
                let edge = if self.pos[0] < x0 { x0 } else { x1 };
                self.pos[0] = 2.0 * edge - self.pos[0];
                self.vel[0] = -self.vel[0] * (0.75 + 0.2 * self.rng.next());
                self.heading = std::f32::consts::PI - self.heading + jitter;
                hit = true;
            }
            if self.pos[1] < y0 || self.pos[1] > y1 {
                let edge = if self.pos[1] < y0 { y0 } else { y1 };
                self.pos[1] = 2.0 * edge - self.pos[1];
                self.vel[1] = -self.vel[1] * (0.75 + 0.2 * self.rng.next());
                self.heading = -self.heading + jitter;
                hit = true;
            }
            self.pos = [self.pos[0].clamp(x0, x1), self.pos[1].clamp(y0, y1)];
            // Dashed into a wall: sometimes it hangs back for a moment.
            if hit && self.pace == Pace::Lunge && !self.talking(now) && self.rng.next() < 0.4 {
                self.pace = Pace::Stall;
                self.speed = self.r * 0.15;
                self.until = now + 0.4 + 0.8 * self.rng.next();
            }
        }
    }

    /// Bit at time `t` (seconds since it set off), or `None` once it has left.
    pub fn at(&mut self, t: f32) -> Option<Frame> {
        self.advance(t);
        // A gentle bob about its path, and breathing in and out a little, as
        // if drifting nearer and farther.
        let w = self.r * WOBBLE;
        let pos = [
            self.pos[0] + (1.8 * t).sin() * w,
            self.pos[1] + (1.4 * t + 1.0).sin() * w,
        ];
        let pos = self.held.unwrap_or(pos);
        let size = 1.0 + (BREATH - 1.0) * (0.37 * t).sin();
        if self.leaving {
            let rad = self.r * size * bit::reach() + 4.0;
            let seen = self.monitors.iter().any(|m| {
                pos[0] + rad > m.x
                    && pos[0] - rad < m.x + m.w
                    && pos[1] + rad > m.y
                    && pos[1] - rad < m.y + m.h
            });
            if !seen {
                return None;
            }
        }
        let morph = |since: f32| (since / MORPH).clamp(0.0, 1.0);
        let (before, mood, u) = match &self.talk {
            Some(k) if t >= k.start + k.hold + MORPH => (Mood::Neutral, Mood::Neutral, 1.0),
            Some(k) if t >= k.start + k.hold => {
                (k.mood, Mood::Neutral, morph(t - k.start - k.hold))
            }
            Some(k) if t >= k.start => (Mood::Neutral, k.mood, morph(t - k.start)),
            _ => (Mood::Neutral, Mood::Neutral, 1.0),
        };
        Some(Frame {
            pos,
            r: self.r * size,
            before,
            mood,
            u,
        })
    }
}

/// `p` moved to the nearest place a Bit of radius `rad` is wholly on one of
/// `screens`.
fn keep_on(screens: &[Monitor], p: [f32; 2], rad: f32) -> [f32; 2] {
    let inside = |v: f32, lo: f32, len: f32| {
        if len < 2.0 * rad {
            lo + len * 0.5
        } else {
            v.clamp(lo + rad, lo + len - rad)
        }
    };
    screens
        .iter()
        .map(|m| [inside(p[0], m.x, m.w), inside(p[1], m.y, m.h)])
        .min_by(|a, b| dist(*a, p).total_cmp(&dist(*b, p)))
        .unwrap_or(p)
}

/// Whether a flight from `p` (at `size`) to `q` (at `q_size`) keeps a Bit of
/// radius `rad` (times its size) off every screen in `avoid`.
fn clear(avoid: &[Monitor], p: [f32; 2], size: f32, q: [f32; 2], q_size: f32, rad: f32) -> bool {
    (0..=48).all(|k| {
        let u = k as f32 / 48.0;
        let c = lerp(p, q, u);
        let r = rad * (size + (q_size - size) * u) + 4.0;
        !avoid.iter().any(|m| {
            c[0] + r > m.x && c[0] - r < m.x + m.w && c[1] + r > m.y && c[1] - r < m.y + m.h
        })
    })
}

/// Where on screen `m` Bit hovers, for the "Hangs out" setting: mostly in
/// that part of the screen, now and then anywhere.
fn spot(m: Monitor, choice: u8, margin: f32, d: &mut impl FnMut() -> f32) -> [f32; 2] {
    let (mut x0, mut x1, mut y0, mut y1) = (0.15, 0.85, 0.2, 0.8);
    if d() < 0.8 {
        match choice {
            SPOT_CENTER => (x0, x1, y0, y1) = (0.35, 0.65, 0.35, 0.65),
            SPOT_TOP => (y0, y1) = (0.1, 0.3),
            SPOT_BOTTOM => (y0, y1) = (0.7, 0.9),
            SPOT_LEFT => (x0, x1) = (0.08, 0.25),
            SPOT_RIGHT => (x0, x1) = (0.75, 0.92),
            SPOT_CORNERS => {
                let (right, below) = (d() < 0.5, d() < 0.5);
                (x0, x1) = if right { (0.78, 0.92) } else { (0.08, 0.22) };
                (y0, y1) = if below { (0.75, 0.9) } else { (0.1, 0.25) };
            }
            _ => {}
        }
    }
    let inside =
        |v: f32, lo: f32, len: f32| v.clamp(lo + margin, (lo + len - margin).max(lo + margin));
    [
        inside(m.x + m.w * (x0 + (x1 - x0) * d()), m.x, m.w),
        inside(m.y + m.h * (y0 + (y1 - y0) * d()), m.y, m.h),
    ]
}

/// Push `p` further away from `origin` until a Bit of radius `rad` there is
/// off every screen.
fn out_of_sight(monitors: &[Monitor], mut p: [f32; 2], origin: [f32; 2], rad: f32) -> [f32; 2] {
    let seen = |p: [f32; 2]| {
        monitors.iter().any(|m| {
            p[0] + rad > m.x && p[0] - rad < m.x + m.w && p[1] + rad > m.y && p[1] - rad < m.y + m.h
        })
    };
    let (dx, dy) = (p[0] - origin[0], p[1] - origin[1]);
    let l = (dx * dx + dy * dy).sqrt();
    let dir = if l > 1.0 {
        [dx / l, dy / l]
    } else {
        [0.0, -1.0]
    };
    let step = monitors.iter().map(|m| m.w.max(m.h)).fold(400.0, f32::max) * 0.1;
    for _ in 0..200 {
        if !seen(p) {
            break;
        }
        p = [p[0] + dir[0] * step, p[1] + dir[1] * step];
    }
    p
}

fn centre(m: &Monitor) -> [f32; 2] {
    [m.x + m.w * 0.5, m.y + m.h * 0.5]
}

fn lerp(a: [f32; 2], b: [f32; 2], u: f32) -> [f32; 2] {
    [a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u]
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With "Main only" or "All but main", Bit is never seen on the other
    /// screens, from its first frame to its last (whether it leaves on its
    /// own or is shooed).
    #[test]
    fn keeps_off_excluded_screens() {
        let main = Monitor {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            primary: true,
        };
        let side = Monitor {
            x: 1920.0,
            y: 201.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            primary: false,
        };
        let above = Monitor {
            x: 0.0,
            y: -1080.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            primary: false,
        };
        let monitors = vec![main, side, above];
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for (screens, avoid) in [
            (SCREENS_MAIN, vec![side, above]),
            (SCREENS_NOT_MAIN, vec![main]),
            (SCREENS_CHOSEN, vec![main, above]),
        ] {
            for spot in 0..=6 {
                let s = Settings {
                    screens,
                    spot,
                    chosen_screens: vec![side.key()],
                    ..Settings::default()
                };
                for n in 0..150 {
                    let mut v = Visit::new(&mut rng, monitors.clone(), &s, None, n % 5 == 0);
                    let shoo_at = if n % 3 == 0 { Some(v.stay * 0.5) } else { None };
                    let mut t = 0.0;
                    while let Some(f) = v.at(t) {
                        let r = f.r * bit::reach();
                        for m in &avoid {
                            let seen = f.pos[0] + r > m.x
                                && f.pos[0] - r < m.x + m.w
                                && f.pos[1] + r > m.y
                                && f.pos[1] - r < m.y + m.h;
                            assert!(!seen, "seen on an excluded screen at t={t} (screens={screens}, spot={spot})");
                        }
                        if shoo_at.is_some_and(|s| t >= s) {
                            v.shoo(t);
                        }
                        t += 1.0 / 60.0;
                    }
                }
            }
        }
    }

    /// Dragged, Bit stays on the screens it's allowed, and dropped it
    /// carries on from there and never leaves across an excluded screen.
    #[test]
    fn dragging() {
        let main = Monitor { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0, primary: true };
        let side = Monitor { x: 1920.0, y: 201.0, w: 1920.0, h: 1080.0, scale: 1.0, primary: false };
        let s = Settings { screens: SCREENS_MAIN, ..Settings::default() };
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for stage in [0.3f32, 3.0, 7.5] {
            let mut v = Visit::new(&mut rng, vec![main, side], &s, None, false);
            let t = stage.min(v.stay - 0.1);
            v.hold(Some([3000.0, 500.0]));
            let f = v.at(t).unwrap();
            let r = f.r * bit::reach();
            assert!(f.pos[0] + r <= 1920.5, "dragged off its screen: {:?}", f.pos);
            v.place(t, [100.0, 100.0]);
            let mut c = t;
            let mut last = None;
            while let Some(f) = v.at(c) {
                let r = f.r * bit::reach();
                let seen = f.pos[0] + r > side.x && f.pos[1] + r > side.y && f.pos[1] - r < side.y + side.h;
                assert!(!seen, "left over the side screen at {c}");
                last = Some(f.pos);
                c += 1.0 / 60.0;
            }
            assert!(last.is_some());
        }
        let hover = Settings { always_hover: true, ..Settings::default() };
        let mut roam = Roam::new(Rng(5), vec![main], &hover);
        let mut t = 0.0;
        while t < 6.0 { roam.at(t); t += 1.0 / 60.0; }
        assert!(roam.grabbable());
        roam.hold(Some([-500.0, 300.0]));
        assert!(roam.at(t).unwrap().pos[0] > 0.0);
        roam.place([400.0, 300.0]);
        assert_eq!(roam.hovering(), Some([400.0, 300.0]));
    }

    /// A long answer plays the long recording of that answer.
    #[test]
    fn long_answers() {
        let m = Monitor { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0, primary: true };
        let s = Settings::default();
        let mut rng = Rng(0x1234_5678_9abc_def1);
        for (forced, mood, voice) in [
            (Voice::YesYesYes, Mood::Yes, Voice::YesYesYes),
            (Voice::NoNoNo, Mood::No, Voice::NoNoNo),
            (Voice::Yes, Mood::Yes, Voice::Yes),
        ] {
            let v = Visit::new(&mut rng, vec![m], &s, Some(forced), false);
            assert_eq!(v.voice(mood), Some(voice));
        }
    }

    /// Hovering, Bit flies in, stays by its spot (centre for Center),
    /// comes back after being startled, and leaves when told to.
    #[test]
    fn hovers_in_one_spot() {
        let m = Monitor {
            x: 1920.0,
            y: 201.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            primary: false,
        };
        for spot in 0..=6 {
            let s = Settings {
                always_hover: true,
                spot,
                always_screen: m.key(),
                ..Settings::default()
            };
            let mut roam = Roam::new(Rng(0x1234_5678_9abc_def1), vec![m], &s);
            let h = roam.hovering().unwrap();
            if spot == SPOT_CENTER {
                assert_eq!(h, centre(&m));
            }
            let reach = roam.r * (HOVER_DRIFT + WOBBLE) + 3.0;
            let mut t = 0.0;
            while t < 60.0 {
                let f = roam.at(t).unwrap();
                let settled = t > 6.0 && !(20.0..28.0).contains(&t);
                assert!(
                    !settled || dist(f.pos, h) <= reach * 1.5,
                    "strayed at t={t}: {:?} from {h:?}",
                    f.pos
                );
                if (20.0..20.02).contains(&t) {
                    roam.startle(t);
                }
                t += 1.0 / 60.0;
            }
            roam.leave();
            let gone = t + 6.0;
            while roam.at(t).is_some() {
                t += 1.0 / 60.0;
                assert!(t < gone, "took too long to leave");
            }
        }
    }

    /// Always on: once it's on its screen, Bit stays wholly on it however
    /// it lunges, stalls or answers; told to leave, it's gone in a moment.
    #[test]
    fn roams_one_screen() {
        let main = Monitor {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
            scale: 1.0,
            primary: true,
        };
        let side = Monitor {
            x: 1920.0,
            y: 201.0,
            w: 1280.0,
            h: 720.0,
            scale: 1.25,
            primary: false,
        };
        let mut rng = Rng(0x0dd_ba11_cafe_f00d);
        for (n, screen) in [main, side, main, side].into_iter().enumerate() {
            let s = Settings {
                always_screen: screen.key(),
                restlessness: [0, 50, 100, 80][n],
                size_pixels: [140.0, 320.0, 60.0, 200.0][n],
                ..Settings::default()
            };
            let mut roam = Roam::new(rng.split(), vec![main, side], &s);
            assert_eq!(roam.screen().key(), screen.key());
            let m = screen;
            let mut on = false;
            let mut closest = f32::MAX;
            let mut t = 0.0;
            while t < 120.0 {
                let f = roam.at(t).expect("still roaming");
                // The plain ball goes right up to the edges; an answer's
                // spikes stay on screen too (allowing a sliver as they grow).
                let plain = f.mood == Mood::Neutral && f.before == Mood::Neutral;
                let r = if plain {
                    f.r * bit::neutral_reach()
                } else {
                    f.r * (bit::reach() - 0.15)
                };
                let inside = f.pos[0] - r >= m.x - 0.5
                    && f.pos[0] + r <= m.x + m.w + 0.5
                    && f.pos[1] - r >= m.y - 0.5
                    && f.pos[1] + r <= m.y + m.h + 0.5;
                if on {
                    assert!(inside, "left its screen at t={t} ({:?})", f.pos);
                    if plain {
                        let gap = (f.pos[0] - r - m.x)
                            .min(m.x + m.w - f.pos[0] - r)
                            .min(f.pos[1] - r - m.y)
                            .min(m.y + m.h - f.pos[1] - r);
                        closest = closest.min(gap);
                    }
                }
                on |= inside && t > 0.1;
                if (t * 60.0) as u32 % 900 == 0 {
                    roam.answer(t, Mood::Yes, n == 3);
                }
                if (t * 60.0) as u32 % 700 == 0 {
                    roam.startle(t);
                }
                t += 1.0 / 60.0;
            }
            assert!(on, "never arrived");
            assert!(
                closest < 12.0,
                "never came near an edge (closest {closest} px)"
            );
            roam.leave();
            let gone = t + 6.0;
            while roam.at(t).is_some() {
                t += 1.0 / 60.0;
                assert!(t < gone, "took too long to leave");
            }
        }
    }
}
