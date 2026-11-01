//! Windows: the settings window runs as a short-lived process of its own
//! (`tron-bit.exe --settings`), started when it's opened. When it closes,
//! everything it loaded goes with it, including the graphics driver, which
//! Windows never unloads from a process once it's in (about 60 MB that
//! would otherwise stay for good).
//!
//! The two processes talk over the window's standard input and output, one
//! line per message, fields split by `SEP`:
//!
//! - Bit to the window: `status` (what Bit is up to), `settings` (changed
//!   from the tray, or by double-clicking Bit).
//! - The window to Bit: `settings` (changed in the window), `ask`, `fly`,
//!   `try` and a voice, `quit`.
//!
//! The window quits by itself if Bit goes away (its input closes).

use crate::flight::Voice;
use crate::settings::{self, Settings};
use crate::{platform, shared, Cmd, Shared, Status};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const SEP: char = '\u{1f}';
/// How often Bit tells the window what it's up to.
const STATUS_EVERY: Duration = Duration::from_millis(250);

// ---- Bit's side ----

struct Open {
    child: Child,
    /// Settings for the window, sent with the next status.
    pending: Option<Settings>,
    /// The settings as the window has them (nothing to send if they match).
    from_window: Option<Settings>,
}

static WINDOW: Mutex<Option<Open>> = Mutex::new(None);

/// Open the settings window, or bring it to the front if it's open.
pub fn open() {
    let mut w = WINDOW.lock().unwrap();
    if let Some(open) = w.as_mut() {
        if matches!(open.child.try_wait(), Ok(None)) {
            platform::raise_window_of(open.child.id());
            return;
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Ok(mut child) = Command::new(exe)
        .arg("--settings")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    else {
        return;
    };
    // Let its window come to the front (the tray click gave us the right).
    platform::allow_foreground(child.id());
    let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.kill();
        return;
    };
    let id = child.id();
    let start = shared().settings.lock().unwrap().clone();
    *w = Some(Open {
        child,
        pending: Some(start.clone()),
        from_window: Some(start),
    });
    drop(w);
    std::thread::spawn(move || tell(input, id));
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            heard(&line);
        }
        // If it closed while waiting for a new key, the old one comes back.
        crate::CHOOSING_KEY.store(false, Ordering::Relaxed);
        platform::tell_settings_changed();
        // Closed: forget it (unless a newer one has opened since).
        let mut w = WINDOW.lock().unwrap();
        if w.as_ref().is_some_and(|o| o.child.id() == id) {
            if let Some(mut o) = w.take() {
                let _ = o.child.wait();
            }
        }
    });
}

/// Settings changed here (tray, double-click): pass them to the window.
pub fn push() {
    let s = shared().settings.lock().unwrap().clone();
    if let Some(open) = WINDOW.lock().unwrap().as_mut() {
        if open.from_window.as_ref() != Some(&s) {
            open.pending = Some(s.clone());
            open.from_window = Some(s);
        }
    }
}

/// Close the window (Bit is quitting).
pub fn close() {
    if let Some(mut open) = WINDOW.lock().unwrap().take() {
        let _ = open.child.kill();
        let _ = open.child.wait();
    }
}

/// Keep the window told what Bit is up to (and of new settings) until it
/// closes.
fn tell(mut input: ChildStdin, id: u32) {
    loop {
        let pending = {
            let mut w = WINDOW.lock().unwrap();
            match w.as_mut() {
                Some(o) if o.child.id() == id => o.pending.take(),
                _ => return,
            }
        };
        let mut text = status_line(&shared().status.lock().unwrap());
        if let Some(s) = pending {
            text.push_str(&settings_line(&s));
        }
        if input.write_all(text.as_bytes()).is_err() || input.flush().is_err() {
            return;
        }
        std::thread::sleep(STATUS_EVERY);
    }
}

/// A message from the window.
fn heard(line: &str) {
    let s = shared();
    let (kind, rest) = line.split_once(SEP).unwrap_or((line, ""));
    match kind {
        "settings" => {
            let text = rest;
            let new = settings::parse(&text.replace(SEP, "\n"));
            let reschedule = {
                let mut cur = s.settings.lock().unwrap();
                let r = new.paused != cur.paused
                    || new.min_interval_seconds != cur.min_interval_seconds
                    || new.max_interval_seconds != cur.max_interval_seconds;
                *cur = new.clone();
                r
            };
            if let Some(open) = WINDOW.lock().unwrap().as_mut() {
                open.from_window = Some(new);
            }
            s.send(if reschedule { Cmd::Reschedule } else { Cmd::Mode });
            // The tray's ticks are the main thread's to change.
            platform::tell_settings_changed();
        }
        "choosing" => {
            crate::CHOOSING_KEY.store(rest == "1", Ordering::Relaxed);
            platform::tell_settings_changed();
        }
        "ask" => s.send(Cmd::Ask),
        "fly" => s.send(Cmd::Visit(None)),
        "try" => s.send(Cmd::Visit(voice(rest))),
        "quit" => platform::ask_main_to_quit(),
        _ => {}
    }
}

fn status_line(st: &Status) -> String {
    let next = st.next_visit.map_or(String::new(), |at| {
        at.saturating_duration_since(Instant::now())
            .as_secs_f32()
            .to_string()
    });
    let clean = |t: &str| t.replace([SEP, '\n'], " ");
    format!(
        "status{SEP}{}{SEP}{}{SEP}{next}{SEP}{}{SEP}{}{SEP}{}\n",
        u8::from(st.visiting),
        u8::from(st.roaming),
        clean(st.held.as_deref().unwrap_or("")),
        clean(st.audio_error.as_deref().unwrap_or("")),
        u8::from(crate::HOTKEY.load(Ordering::Relaxed)),
    )
}

fn settings_line(s: &Settings) -> String {
    format!("settings{SEP}{}\n", settings::to_text(s).replace('\n', &SEP.to_string()))
}

fn voice(v: &str) -> Option<Voice> {
    match v {
        "yes" => Some(Voice::Yes),
        "no" => Some(Voice::No),
        "nonono" => Some(Voice::NoNoNo),
        "yesyesyes" => Some(Voice::YesYesYes),
        _ => None,
    }
}

// ---- The window's side ----

static CHILD: AtomicBool = AtomicBool::new(false);

/// This process is the settings window.
pub fn is_child() -> bool {
    CHILD.load(Ordering::Relaxed)
}

/// Run as the settings window, and exit when it closes.
pub fn run_child() -> ! {
    CHILD.store(true, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel();
    let state = Arc::new(Shared {
        settings: Mutex::new(settings::load()),
        status: Mutex::new(Status::default()),
        cmd: Mutex::new(tx),
        egui: Mutex::new(None),
        settings_open: AtomicBool::new(false),
    });
    let _ = crate::SHARED.set(state.clone());
    // What the window asks of Bit.
    std::thread::spawn(move || {
        for cmd in rx {
            let line = match cmd {
                Cmd::Ask => "ask".to_string(),
                Cmd::Visit(None) => "fly".to_string(),
                Cmd::Visit(Some(v)) => format!(
                    "try{SEP}{}",
                    match v {
                        Voice::Yes => "yes",
                        Voice::No => "no",
                        Voice::NoNoNo => "nonono",
                        Voice::YesYesYes => "yesyesyes",
                    }
                ),
                Cmd::Quit => "quit".to_string(),
                Cmd::Reschedule | Cmd::Mode => continue,
            };
            say(&line);
        }
    });
    // What Bit tells the window; when Bit goes, so does the window.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let f: Vec<&str> = line.split(SEP).collect();
            match f.as_slice() {
                ["status", visiting, roaming, next, held, audio, hotkey] => {
                    let mut st = state.status.lock().unwrap();
                    st.visiting = *visiting == "1";
                    st.roaming = *roaming == "1";
                    st.next_visit = next
                        .parse::<f32>()
                        .ok()
                        .map(|s| Instant::now() + Duration::from_secs_f32(s));
                    st.held = (!held.is_empty()).then(|| held.to_string());
                    st.audio_error = (!audio.is_empty()).then(|| audio.to_string());
                    crate::HOTKEY.store(*hotkey == "1", Ordering::Relaxed);
                }
                ["settings", ..] => {
                    let text = line["settings".len() + SEP.len_utf8()..].replace(SEP, "\n");
                    *state.settings.lock().unwrap() = settings::parse(&text);
                }
                _ => {}
            }
        }
        std::process::exit(0);
    });
    crate::run_settings();
    std::process::exit(0);
}

/// Settings changed in the window: tell Bit.
pub fn send_settings() {
    say(&settings_line(&shared().settings.lock().unwrap()).trim_end().to_string());
}

/// Waiting for a new Ask Bit key, or done: tell Bit.
pub fn tell_choosing_key(on: bool) {
    say(&format!("choosing{SEP}{}", u8::from(on)));
}

/// Quit Bit (the QUIT BIT button), then close.
pub fn quit_bit() -> ! {
    say("quit");
    std::process::exit(0);
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
