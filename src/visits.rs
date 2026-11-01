//! Bit's own thread: it sleeps until the next visit, then flies one visit
//! from a single clock (motion, drawing and voice all read the same time),
//! one frame per display refresh. With Always on, it roams its screen
//! instead, until that's switched off.

use crate::audio::{self, Sounds, Speaker};
use crate::bit::{self, Canvas, Mood, Pose};
use crate::flight::{Frame, Rng, Roam, Visit, Voice};
use crate::overlay::Overlay;
use crate::settings::{self, Settings, CLICK_ASK, CLICK_NOTHING};
use crate::{platform, Cmd, Shared};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub fn run(shared: Arc<Shared>, rx: Receiver<Cmd>) {
    let mut rng = Rng::seeded();
    let mut sounds = Sounds::load();
    let mut canvas = Canvas::new();
    let mut next = schedule(&shared, &mut rng);
    let mut was_always = false;
    loop {
        let s = shared.settings.lock().unwrap().clone();
        if s.always_on && !s.paused {
            // Always on: Bit roams until that's switched off (or visits are
            // paused), and keeps away during quiet hours, calls and full
            // screen. No visits are due meanwhile.
            was_always = true;
            let flight = match held(&shared) {
                None => match roam(&shared, &rx, &mut rng, &mut sounds, &mut canvas) {
                    Ended::Quit => return,
                    // Asked as it was leaving: it answers on a visit.
                    Ended::Asked => Some((Some(answer()), true)),
                    Ended::Done => None,
                    Ended::NoWindow => match rx.recv_timeout(Duration::from_secs(5)) {
                        Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                        _ => None,
                    },
                },
                Some((wait, why)) => {
                    if let Ok(mut st) = shared.status.lock() {
                        st.next_visit = None;
                        st.held = Some(why);
                    }
                    // Look again now and then: the wait may end early.
                    match rx.recv_timeout(wait.min(Duration::from_secs(30))) {
                        Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                        // Fly in and Ask Bit still work while it keeps away.
                        Ok(Cmd::Visit(voice)) => Some((voice, false)),
                        Ok(Cmd::Ask) => Some((Some(answer()), true)),
                        _ => None,
                    }
                }
            };
            if let Some((voice, ask)) = flight {
                if visit(&shared, &rx, &mut rng, &mut sounds, &mut canvas, voice, ask) {
                    return;
                }
            }
            continue;
        }
        if was_always {
            // Back to visits: the first comes after the usual wait.
            was_always = false;
            next = schedule(&shared, &mut rng);
        }
        let cmd = match next {
            Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                Ok(c) => Some(c),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(c) => Some(c),
                Err(_) => return,
            },
        };
        let (forced, ask) = match cmd {
            Some(Cmd::Quit) => return,
            Some(Cmd::Reschedule) => {
                next = schedule(&shared, &mut rng);
                continue;
            }
            // (Always on may have been switched on: look again.)
            Some(Cmd::Mode) => continue,
            Some(Cmd::Visit(voice)) => (voice, false),
            Some(Cmd::Ask) => (Some(answer()), true),
            None => match held(&shared) {
                // Not now: look again later.
                Some((wait, why)) => {
                    next = Some(Instant::now() + wait);
                    if let Ok(mut st) = shared.status.lock() {
                        st.next_visit = next;
                        st.held = Some(why);
                    }
                    continue;
                }
                None => (None, false),
            },
        };
        if visit(&shared, &rx, &mut rng, &mut sounds, &mut canvas, forced, ask) {
            return;
        }
        next = schedule(&shared, &mut rng);
    }
}

/// One visit, plus another straight after for each question asked during
/// it. True if Bit is to quit.
fn visit(
    shared: &Arc<Shared>,
    rx: &Receiver<Cmd>,
    rng: &mut Rng,
    sounds: &mut Sounds,
    canvas: &mut Canvas,
    voice: Option<Voice>,
    ask: bool,
) -> bool {
    let mut flight = Some((voice, ask));
    while let Some((voice, ask)) = flight.take() {
        match fly(shared, rx, rng, sounds, canvas, voice, ask) {
            Ended::Quit => return true,
            // Asked a question while busy: come straight back and answer.
            Ended::Asked => flight = Some((Some(answer()), true)),
            Ended::Done | Ended::NoWindow => {}
        }
    }
    false
}

/// The magic eight ball's answer: one bit from the operating system's
/// cryptographic random number generator (seeded from hardware noise), so
/// yes and no are exactly even and nothing before predicts the next one.
fn answer() -> Voice {
    let mut b = [0u8; 1];
    if getrandom::fill(&mut b).is_err() {
        // No OS generator (never expected): fall back to the clock.
        b[0] = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos() as u8);
    }
    if b[0] & 1 == 1 {
        Voice::Yes
    } else {
        Voice::No
    }
}

fn mood_of(v: Voice) -> Mood {
    if matches!(v, Voice::Yes | Voice::YesYesYes) {
        Mood::Yes
    } else {
        Mood::No
    }
}

enum Ended {
    Done,
    /// Asked a question during the visit.
    Asked,
    Quit,
    /// No window to draw Bit in.
    NoWindow,
}

/// Whether a visit that's due should wait, for how long, and why (for the
/// settings window): quiet hours, a call, or a full-screen app.
fn held(shared: &Shared) -> Option<(Duration, String)> {
    let s = shared.settings.lock().unwrap().clone();
    let now = platform::local_minutes();
    if let Some(left) = s.quiet_left(now) {
        let end = (now + left) % settings::DAY;
        let why = format!("Quiet hours until {:02}:{:02}", end / 60, end % 60);
        // Wake at the end of quiet hours (a minute in, so the clock agrees).
        return Some((Duration::from_secs(u64::from(left) * 60 + 30), why));
    }
    if s.quiet_during_calls && platform::mic_in_use() {
        return Some((
            Duration::from_secs(30),
            "Waiting for your call to end".into(),
        ));
    }
    if platform::busy() {
        // Someone's in a full-screen game or presentation.
        return Some((
            Duration::from_secs(60),
            "Waiting for full screen to end".into(),
        ));
    }
    None
}

/// Pick the next visit time (none while paused) and tell the settings window.
fn schedule(shared: &Shared, rng: &mut Rng) -> Option<Instant> {
    let s = shared.settings.lock().unwrap().clone();
    let next = (!s.paused).then(|| {
        let span = s
            .max_interval_seconds
            .saturating_sub(s.min_interval_seconds) as f32;
        Instant::now() + Duration::from_secs_f32(s.min_interval_seconds as f32 + span * rng.next())
    });
    set_next(shared, next);
    next
}

fn set_next(shared: &Shared, next: Option<Instant>) {
    if let Ok(mut st) = shared.status.lock() {
        st.next_visit = next;
        st.held = None;
    }
}

/// Open the speaker, unless it's open already or Bit is to keep quiet on a
/// call (then it only shows its answer).
fn open_speaker(shared: &Shared, speaker: &mut Option<Speaker>, sounds: &mut Sounds) {
    let quiet = shared.settings.lock().unwrap().quiet_during_calls;
    if speaker.is_some() || (quiet && platform::mic_in_use()) {
        return;
    }
    let sp = Speaker::open(sounds);
    if let Ok(mut st) = shared.status.lock() {
        st.audio_error = sp.as_ref().err().cloned();
    }
    *speaker = sp.ok();
}

/// Say `v`. An answer to a question (`aloud`) is always said, even with
/// sound switched off (the settings are only overridden, never changed);
/// otherwise the sound settings decide.
fn speak(sp: &Speaker, v: Voice, s: &Settings, aloud: bool) {
    let on = match v {
        Voice::Yes => s.yes_sound_enabled,
        Voice::No => s.no_sound_enabled,
        Voice::NoNoNo => s.alarm_sound_enabled,
        Voice::YesYesYes => s.yes3_sound_enabled,
    };
    if aloud {
        // Muted volume would hide the answer: use the default level.
        let volume = if s.volume_percent > 0 {
            s.volume_percent
        } else {
            70
        };
        sp.play(v, audio::LEVEL * f32::from(volume) / 100.0);
    } else if s.sound_enabled && on {
        sp.play(v, audio::LEVEL * f32::from(s.volume_percent) / 100.0);
    }
}

/// Draw Bit as `f` has it (`t` turns its tumble) and put it on screen: a
/// bitmap just big enough for Bit, placed to the whole pixel, with Bit drawn
/// at its exact sub-pixel spot inside it.
fn show(canvas: &mut Canvas, overlay: &mut Overlay, f: &Frame, t: f32) {
    let side = (2.0 * f.r * bit::reach()).ceil() as usize + 4;
    let left = (f.pos[0] - side as f32 * 0.5).floor();
    let top = (f.pos[1] - side as f32 * 0.5).floor();
    let pose = Pose {
        t,
        before: f.before,
        mood: f.mood,
        u: f.u,
    };
    let pixels = canvas.render(side, f.pos[0] - left, f.pos[1] - top, f.r, &pose);
    overlay.present(left as i32, top as i32, side, pixels);
}

/// Low power: a new frame about every 33 ms (still in step with the display)
/// rather than at every refresh.
const LOW_POWER_FRAME: f32 = 0.031;

/// Wait for the display to show the frame, and for at least `frame_time`
/// seconds in all.
fn wait(overlay: &Overlay, frame_time: f32) {
    let shown = Instant::now();
    loop {
        overlay.wait_frame();
        if shown.elapsed().as_secs_f32() >= frame_time {
            break;
        }
    }
}

/// Let the last word finish (or fade it out, quitting) before the speaker
/// closes.
fn finish_speaking(
    speaker: Option<Speaker>,
    rx: &Receiver<Cmd>,
    quit: &mut bool,
    asked: &mut bool,
) {
    let Some(sp) = speaker else { return };
    let until = Instant::now() + Duration::from_secs(4);
    while !sp.idle() && Instant::now() < until {
        match rx.try_recv() {
            Ok(Cmd::Quit) => *quit = true,
            Ok(Cmd::Ask) => *asked = true,
            _ => {}
        }
        if *quit {
            sp.fade_out();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The clock of a flight, which stops while Bit is held.
struct Clock {
    start: Instant,
    stopped: Duration,
    since: Option<Instant>,
}

impl Clock {
    fn new() -> Clock {
        Clock {
            start: Instant::now(),
            stopped: Duration::ZERO,
            since: None,
        }
    }

    fn now(&self) -> f32 {
        let at = self.since.unwrap_or_else(Instant::now);
        (at - self.start - self.stopped).as_secs_f32()
    }

    fn hold(&mut self, held: bool) {
        match (held, self.since) {
            (true, None) => self.since = Some(Instant::now()),
            (false, Some(at)) => {
                self.stopped += at.elapsed();
                self.since = None;
            }
            _ => {}
        }
    }
}

/// What the pointer did to Bit this frame.
enum Touch {
    Nothing,
    /// Held down, Bit to be here.
    Held([f32; 2]),
    Clicked,
    /// Dragged and let go here.
    Dropped([f32; 2]),
}

/// Follows a press on Bit: pressed and let go it's a click; moved more than a
/// few pixels first, it's a drag.
#[derive(Default)]
struct Grip {
    held: bool,
    moved: bool,
    at: [f32; 2],
    offset: [f32; 2],
    bit: [f32; 2],
}

impl Grip {
    fn touch(&mut self, down: bool, released: bool, bit: [f32; 2]) -> Touch {
        let cursor = platform::cursor();
        if down && !self.held {
            *self = Grip {
                held: true,
                moved: false,
                at: cursor,
                offset: [bit[0] - cursor[0], bit[1] - cursor[1]],
                bit,
            };
        }
        if !self.held {
            return if released { Touch::Clicked } else { Touch::Nothing };
        }
        let (dx, dy) = (cursor[0] - self.at[0], cursor[1] - self.at[1]);
        self.moved |= dx * dx + dy * dy > 25.0;
        let pos = if self.moved {
            [cursor[0] + self.offset[0], cursor[1] + self.offset[1]]
        } else {
            self.bit
        };
        if down {
            Touch::Held(pos)
        } else {
            self.held = false;
            if self.moved {
                Touch::Dropped(pos)
            } else {
                Touch::Clicked
            }
        }
    }
}

/// One visit.
fn fly(
    shared: &Arc<Shared>,
    rx: &Receiver<Cmd>,
    rng: &mut Rng,
    sounds: &mut Sounds,
    canvas: &mut Canvas,
    voice: Option<Voice>,
    ask: bool,
) -> Ended {
    let settings = shared.settings.lock().unwrap().clone();
    let mut visit = Visit::new(rng, platform::monitors(), &settings, voice, ask);
    let clickable = settings.click != CLICK_NOTHING;
    let Some(mut overlay) = Overlay::new(shared, clickable, settings.hide_from_capture) else {
        return Ended::NoWindow;
    };
    if let Ok(mut st) = shared.status.lock() {
        st.visiting = true;
        st.next_visit = None;
    }
    // Open the speaker for this visit only (Bit is still off screen). A
    // question always gets a spoken answer, whatever the sound settings.
    let mut speaker = None;
    if ask || (settings.sound_enabled && settings.volume_percent > 0) {
        open_speaker(shared, &mut speaker, sounds);
    }
    let frame_time = if settings.low_power {
        LOW_POWER_FRAME
    } else {
        0.0
    };
    let spin = rng.next() * 100.0;
    let wall = Instant::now();
    let mut clock = Clock::new();
    let mut grip = Grip::default();
    let mut bit_at = [0.0; 2];
    let mut said = Mood::Neutral;
    let mut quit = false;
    let mut asked = false;
    let mut answering = false;
    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Quit => quit = true,
                Cmd::Ask => asked = true,
                _ => {}
            }
        }
        if quit {
            break;
        }
        let (down, released) = overlay.press();
        let touch = if visit.grabbable() {
            grip.touch(down, released, bit_at)
        } else {
            Touch::Nothing
        };
        if let Touch::Held(_) = touch {
            clock.hold(true);
        } else {
            clock.hold(false);
        }
        let t = clock.now();
        match touch {
            Touch::Nothing => {}
            Touch::Held(p) => visit.hold(Some(p)),
            Touch::Dropped(p) => visit.place(t, p),
            Touch::Clicked => {
                visit.hold(None);
                if settings.click == CLICK_ASK {
                    // Clicked with a question in mind: it stops and answers.
                    if visit.answer(t, mood_of(answer())) {
                        answering = true;
                        open_speaker(shared, &mut speaker, sounds);
                    }
                } else {
                    visit.shoo(t);
                    if let Some(sp) = &speaker {
                        sp.fade_out();
                    }
                }
            }
        }
        let Some(f) = visit.at(t) else { break };
        bit_at = f.pos;
        // Bit speaks each time it changes to "yes" or "no".
        if f.mood != said {
            said = f.mood;
            if let (Some(sp), Some(v)) = (&speaker, visit.voice(f.mood)) {
                let s = shared.settings.lock().unwrap().clone();
                speak(sp, v, &s, ask || answering);
            }
        }
        show(canvas, &mut overlay, &f, spin + wall.elapsed().as_secs_f32());
        wait(&overlay, frame_time);
    }
    drop(overlay);
    // Let go of the drawing buffers: a close pass can grow them to several
    // megabytes, and Bit may not visit again for a while.
    *canvas = Canvas::new();
    finish_speaking(speaker, rx, &mut quit, &mut asked);
    if let Ok(mut st) = shared.status.lock() {
        st.visiting = false;
    }
    if quit {
        Ended::Quit
    } else if asked {
        Ended::Asked
    } else {
        Ended::Done
    }
}

/// Always on: Bit roams its screen until Always on is switched off (in the
/// settings, the tray, or by double-clicking Bit), visits are paused, or
/// quiet hours, a call or full screen send it away. It's always drawn at the
/// low-power frame rate. Now and then (Says something) it says yes or no by
/// itself, as the sound settings allow; asked something, it always answers
/// out loud.
fn roam(
    shared: &Arc<Shared>,
    rx: &Receiver<Cmd>,
    rng: &mut Rng,
    sounds: &mut Sounds,
    canvas: &mut Canvas,
) -> Ended {
    let mut settings = shared.settings.lock().unwrap().clone();
    let mut roam = Roam::new(rng.split(), platform::monitors(), &settings);
    let clickable = settings.click != CLICK_NOTHING;
    let Some(mut overlay) = Overlay::new(shared, clickable, settings.hide_from_capture) else {
        return Ended::NoWindow;
    };
    if let Ok(mut st) = shared.status.lock() {
        st.visiting = true;
        st.roaming = true;
        st.next_visit = None;
        st.held = None;
    }
    // The speaker is open only while Bit is answering.
    let mut speaker = None;
    // The answer is to a question (always said), not a TRY IT.
    let mut aloud = false;
    let double = platform::double_click_seconds();
    let mut click_at: Option<f32> = None;
    let spin = rng.next() * 100.0;
    let wall = Instant::now();
    let mut clock = Clock::new();
    let mut grip = Grip::default();
    let mut bit_at = [0.0; 2];
    let mut said = Mood::Neutral;
    let mut quit = false;
    let mut asked = false;
    let mut checked = 0.0;
    // When it next says something by itself.
    let chatter = |t: f32, s: &Settings, rng: &mut Rng| {
        Settings::chatter_seconds(s.always_chatter).map(|m| t + m * (0.5 + rng.next()))
    };
    let mut next_talk = chatter(0.0, &settings, rng);
    loop {
        let (down, released) = overlay.press();
        let touch = if roam.grabbable() {
            grip.touch(down, released, bit_at)
        } else if released {
            Touch::Clicked
        } else {
            Touch::Nothing
        };
        if let Touch::Held(_) = touch {
            clock.hold(true);
        } else {
            clock.hold(false);
        }
        let t = clock.now();
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Quit => quit = true,
                Cmd::Ask => {
                    if roam.answer(t, mood_of(answer()), false) {
                        aloud = true;
                        open_speaker(shared, &mut speaker, sounds);
                    } else if roam.leaving() {
                        // Answered on a visit once it's gone.
                        asked = true;
                    }
                }
                // TRY IT in the settings: it says that, as the sound
                // settings allow.
                Cmd::Visit(Some(v)) => {
                    if roam.answer(t, mood_of(v), matches!(v, Voice::NoNoNo | Voice::YesYesYes)) {
                        aloud = false;
                        if settings.sound_enabled && settings.volume_percent > 0 {
                            open_speaker(shared, &mut speaker, sounds);
                        }
                    }
                }
                Cmd::Visit(None) => roam.startle(t),
                Cmd::Mode | Cmd::Reschedule => {
                    let s = shared.settings.lock().unwrap().clone();
                    // A change it can't make on the fly: off it goes, and
                    // (if still always on) comes straight back in with it.
                    if !s.always_on
                        || s.paused
                        || s.always_screen != settings.always_screen
                        || s.hide_from_capture != settings.hide_from_capture
                        || (s.click == CLICK_NOTHING) != (settings.click == CLICK_NOTHING)
                    {
                        roam.leave();
                    }
                    roam.update(&s);
                    if s.always_chatter != settings.always_chatter {
                        next_talk = chatter(t, &s, rng);
                    }
                    settings = s;
                }
            }
        }
        if quit {
            break;
        }
        if t - checked >= 2.0 {
            checked = t;
            if let Some((_, why)) = held(shared) {
                if let Ok(mut st) = shared.status.lock() {
                    st.held = Some(why);
                }
                roam.leave();
            }
        }
        match touch {
            Touch::Nothing => {}
            Touch::Held(p) => roam.hold(Some(p)),
            Touch::Dropped(p) => roam.place(p),
            Touch::Clicked => {
                roam.hold(None);
                if click_at.take().is_some() {
                    // Double-clicked: back to normal visits.
                    {
                        let mut s = shared.settings.lock().unwrap();
                        s.always_on = false;
                        settings::save(&s);
                        settings = s.clone();
                    }
                    platform::tell_settings_changed();
                    roam.leave();
                } else {
                    click_at = Some(t);
                }
            }
        }
        if next_talk.is_some_and(|at| t >= at) {
            next_talk = chatter(t, &settings, rng);
            let mood = if rng.next() < 0.55 { Mood::Yes } else { Mood::No };
            let long = if mood == Mood::Yes {
                settings.yes3_sound_enabled
            } else {
                settings.alarm_sound_enabled
            };
            if roam.answer(t, mood, long && rng.next() < 0.2) {
                aloud = false;
                if settings.sound_enabled && settings.volume_percent > 0 {
                    open_speaker(shared, &mut speaker, sounds);
                }
            }
        }
        // A single click, once it's clearly not the start of a double-click.
        if click_at.is_some_and(|c| t - c > double) {
            click_at = None;
            if settings.click == CLICK_ASK {
                if roam.answer(t, mood_of(answer()), false) {
                    aloud = true;
                    open_speaker(shared, &mut speaker, sounds);
                }
            } else {
                roam.startle(t);
            }
        }
        let Some(f) = roam.at(t) else { break };
        bit_at = f.pos;
        if f.mood != said {
            said = f.mood;
            if let (Some(sp), Some(v)) = (&speaker, roam.voice(f.mood)) {
                speak(sp, v, &settings, aloud);
            }
        }
        // Done answering and the last word said: close the speaker.
        if speaker.as_ref().is_some_and(|sp| sp.idle()) && !roam.talking(t) {
            speaker = None;
        }
        show(canvas, &mut overlay, &f, spin + wall.elapsed().as_secs_f32());
        wait(&overlay, LOW_POWER_FRAME);
    }
    drop(overlay);
    *canvas = Canvas::new();
    finish_speaking(speaker, rx, &mut quit, &mut asked);
    if let Ok(mut st) = shared.status.lock() {
        st.visiting = false;
        st.roaming = false;
    }
    if quit {
        Ended::Quit
    } else if asked {
        Ended::Asked
    } else {
        Ended::Done
    }
}
