#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Bit, the yes/no companion from Tron (1982), visiting the desktop now and
//! then. Bit lives on its own thread (src/visits.rs) and draws itself
//! (src/bit.rs); the main thread runs the tray icon and the settings window.

mod audio;
mod bit;
mod flight;
mod overlay;
mod platform;
mod settings;
mod ui;
mod visits;
#[cfg(windows)]
mod window;

use eframe::egui;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub enum Cmd {
    /// Fly in now, saying this (or whatever Bit likes).
    Visit(Option<flight::Voice>),
    /// The visit times or pause changed.
    Reschedule,
    /// Magic eight ball: fly in and answer yes or no.
    Ask,
    /// Some other setting changed (Always on, its screen, size...).
    Mode,
    Quit,
}

#[derive(Default)]
pub struct Status {
    pub next_visit: Option<Instant>,
    pub visiting: bool,
    /// Out roaming (Always on).
    pub roaming: bool,
    pub audio_error: Option<String>,
    /// Why a due visit is waiting (quiet hours, a call...).
    pub held: Option<String>,
}

/// What the main thread and Bit's thread share.
pub struct Shared {
    pub settings: Mutex<settings::Settings>,
    pub status: Mutex<Status>,
    cmd: Mutex<mpsc::Sender<Cmd>>,
    /// The settings window's egui context, while it is open.
    pub egui: Mutex<Option<egui::Context>>,
    pub settings_open: AtomicBool,
    #[cfg(not(windows))]
    pub overlay: Mutex<Option<overlay::OverlayFrame>>,
}

impl Shared {
    pub fn send(&self, cmd: Cmd) {
        if let Ok(tx) = self.cmd.lock() {
            let _ = tx.send(cmd);
        }
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();
static BIT_THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

thread_local! {
    /// The tray icon and its pause and Always on items (they belong to the
    /// thread that made them).
    static TRAY: RefCell<Option<Tray>> = const { RefCell::new(None) };
}

pub fn shared() -> &'static Arc<Shared> {
    SHARED.get().expect("shared state")
}

/// Ctrl+Alt+B is ours (asks Bit a question).
pub static HOTKEY: AtomicBool = AtomicBool::new(false);

/// Ask Bit a yes/no question.
pub fn ask() {
    shared().send(Cmd::Ask);
}

/// Show the settings window (or bring it to the front).
#[cfg(windows)]
pub fn open_settings() {
    window::open();
}

/// Show the settings window (or bring it to the front).
#[cfg(not(windows))]
pub fn open_settings() {
    let s = shared();
    if let Some(ctx) = s.egui.lock().unwrap().as_ref() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.request_repaint();
    }
}

struct Tray {
    _icon: TrayIcon,
    ask: MenuItem,
    pause: CheckMenuItem,
    always: CheckMenuItem,
}

/// Settings changed: keep the tray's ticks in step and tell Bit's thread
/// (`reschedule`: the visit times or pause changed, so pick a new time).
pub fn settings_changed(reschedule: bool) {
    #[cfg(windows)]
    if window::is_child() {
        window::send_settings();
        return;
    }
    sync_tray();
    #[cfg(windows)]
    window::push();
    shared().send(if reschedule { Cmd::Reschedule } else { Cmd::Mode });
}

/// Settings changed on another thread (Bit's, double-clicked out of Always
/// on, or the settings window's): tick the tray to match, and tell the
/// window.
fn changed_elsewhere() {
    bind_ask_key();
    sync_tray();
    #[cfg(windows)]
    window::push();
}

/// The Ask Bit key now bound.
static BOUND_KEY: Mutex<String> = Mutex::new(String::new());
/// The settings window is waiting for a new key: the old one is let go
/// meanwhile, or pressing it would ask Bit and never reach the window.
pub static CHOOSING_KEY: AtomicBool = AtomicBool::new(false);

/// Use the Ask Bit key in the settings, if it has changed.
fn bind_ask_key() {
    let choosing = CHOOSING_KEY.load(Ordering::Relaxed);
    let want = if choosing {
        String::new()
    } else {
        shared().settings.lock().unwrap().ask_key.clone()
    };
    let mut bound = BOUND_KEY.lock().unwrap();
    if *bound != want {
        let ok = platform::set_hotkey(&want);
        if !choosing {
            HOTKEY.store(ok, Ordering::Relaxed);
        }
        *bound = want;
    }
}

/// The tray's Ask Bit item, showing the key if it works.
fn ask_label() -> String {
    let bound = BOUND_KEY.lock().unwrap();
    if HOTKEY.load(Ordering::Relaxed) && !bound.is_empty() {
        format!("Ask Bit  ({bound})")
    } else {
        "Ask Bit".to_string()
    }
}

/// Tick the tray's items to match the settings.
fn sync_tray() {
    let s = shared();
    let (paused, always) = {
        let st = s.settings.lock().unwrap();
        (st.paused, st.always_on)
    };
    TRAY.with(|t| {
        if let Some(tray) = &*t.borrow() {
            tray.pause.set_checked(paused);
            tray.always.set_checked(always);
            tray.ask.set_text(ask_label());
        }
    });
}

/// The settings window started or stopped waiting for a new Ask Bit key.
pub fn choosing_key(on: bool) {
    #[cfg(windows)]
    window::tell_choosing_key(on);
    #[cfg(not(windows))]
    let _ = on;
}

/// Stop everything in order and leave: Bit's thread fades out its voice and
/// closes its window, settings are saved, the tray icon is removed.
pub fn quit() -> ! {
    #[cfg(windows)]
    if window::is_child() {
        window::quit_bit();
    }
    #[cfg(windows)]
    window::close();
    let s = shared();
    s.send(Cmd::Quit);
    if let Some(h) = BIT_THREAD.lock().ok().and_then(|mut h| h.take()) {
        let _ = h.join();
    }
    settings::save(&s.settings.lock().unwrap());
    TRAY.with(|t| t.borrow_mut().take());
    std::process::exit(0);
}

fn make_tray() -> Option<Tray> {
    let (paused, always_on) = {
        let st = shared().settings.lock().unwrap();
        (st.paused, st.always_on)
    };
    let pause = CheckMenuItem::with_id("pause", "Pause visits", true, paused, None);
    let ask_item = MenuItem::with_id("ask", ask_label(), true, None);
    let always = CheckMenuItem::with_id("always", "Always on", true, always_on, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &ask_item,
        &MenuItem::with_id("show", "Show Bit now", true, None),
        &pause,
        &always,
        &MenuItem::with_id("settings", "Settings", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("quit", "Quit Bit", true, None),
    ]);
    let icon = tray_icon::Icon::from_rgba(bit::icon_rgba(32), 32, 32).ok()?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("Bit")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()?;
    Some(Tray {
        _icon: tray,
        ask: ask_item,
        pause,
        always,
    })
}

fn on_menu(event: MenuEvent) {
    let s = shared();
    match event.id.0.as_str() {
        "ask" => ask(),
        "show" => s.send(Cmd::Visit(None)),
        "pause" => {
            {
                let mut st = s.settings.lock().unwrap();
                st.paused = !st.paused;
                settings::save(&st);
            }
            settings_changed(true);
        }
        "always" => {
            {
                let mut st = s.settings.lock().unwrap();
                st.always_on = !st.always_on;
                settings::save(&st);
            }
            settings_changed(false);
        }
        "settings" => open_settings(),
        "quit" => quit(),
        _ => {}
    }
}

fn on_tray(event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        open_settings();
    }
}

struct App {
    ui: ui::SettingsUi,
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let s = shared();
        if !self.ui.capturing_key() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            settings::save(&s.settings.lock().unwrap());
            // Elsewhere the window has to stay alive to show Bit's frames,
            // so closing it just tucks it away.
            if cfg!(not(windows)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
        }
        #[cfg(not(windows))]
        show_overlay(ctx, s);
        self.ui.show(ctx, s);
        // 30 fps is plenty for the preview (asking for 60 here makes egui
        // repaint flat out).
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// Elsewhere than Windows, Bit's frames are shown in a transparent,
/// click-through egui viewport.
#[cfg(not(windows))]
fn show_overlay(ctx: &egui::Context, s: &Shared) {
    if let Some(size) = ctx.input(|i| i.viewport().monitor_size) {
        *platform::SCREEN.lock().unwrap() = Some((
            size.x * ctx.pixels_per_point(),
            size.y * ctx.pixels_per_point(),
            ctx.pixels_per_point(),
        ));
    }
    let frame = s.overlay.lock().unwrap();
    let Some(f) = frame.as_ref() else { return };
    let ppp = ctx.pixels_per_point();
    let side = f.side as f32 / ppp;
    let image = egui::ColorImage {
        size: [f.side, f.side],
        pixels: f
            .pixels
            .iter()
            .map(|&p| {
                egui::Color32::from_rgba_premultiplied(
                    (p >> 16) as u8,
                    (p >> 8) as u8,
                    p as u8,
                    (p >> 24) as u8,
                )
            })
            .collect(),
    };
    let builder = egui::ViewportBuilder::default()
        .with_title("Bit")
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_taskbar(false)
        .with_inner_size([side, side])
        .with_position([f.x / ppp, f.y / ppp]);
    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of("bit-overlay"),
        builder,
        move |ctx, _| {
            let tex = ctx.load_texture("bit-frame", image.clone(), egui::TextureOptions::NEAREST);
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    ui.painter().image(
                        tex.id(),
                        ui.max_rect(),
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                });
        },
    );
}

fn main() {
    platform::init();
    #[cfg(windows)]
    if std::env::args().nth(1).as_deref() == Some("--settings") {
        window::run_child();
    }
    let Ok(_lock) = settings::single_instance() else {
        // Bit is already running: open its settings instead.
        platform::signal_first_instance();
        return;
    };
    let first_run = settings::first_run();
    let (tx, rx) = mpsc::channel();
    let state = Arc::new(Shared {
        settings: Mutex::new(settings::load()),
        status: Mutex::new(Status::default()),
        cmd: Mutex::new(tx),
        egui: Mutex::new(None),
        settings_open: AtomicBool::new(false),
        #[cfg(not(windows))]
        overlay: Mutex::new(None),
    });
    let _ = SHARED.set(state.clone());
    let bit = std::thread::Builder::new()
        .name("bit".into())
        .spawn(move || visits::run(state, rx))
        .expect("start Bit");
    *BIT_THREAD.lock().unwrap() = Some(bit);

    // Start at login is on from the first run, and the entry is kept
    // pointing at this copy of the program.
    if first_run || platform::autostart() {
        platform::set_autostart(true);
    }

    let key = shared().settings.lock().unwrap().ask_key.clone();
    HOTKEY.store(
        platform::listen(&key, open_settings, ask, changed_elsewhere, || quit()),
        Ordering::Relaxed,
    );
    *BOUND_KEY.lock().unwrap() = key;
    MenuEvent::set_event_handler(Some(on_menu));
    TrayIconEvent::set_event_handler(Some(on_tray));
    start_tray();

    // On Windows the settings window is a process of its own, started when
    // it's opened (src/window.rs); this thread just handles the tray's
    // messages until Bit quits.
    #[cfg(windows)]
    {
        static NEVER: AtomicBool = AtomicBool::new(false);
        platform::wait_until(&NEVER);
        quit();
    }
    #[cfg(not(windows))]
    {
        run_settings();
        quit();
    }
}

fn run_settings() {
    let s = shared();
    s.settings_open.store(true, Ordering::Relaxed);
    let icon = egui::IconData {
        rgba: bit::icon_rgba(64),
        width: 64,
        height: 64,
    };
    // (Taller while the Chosen screens map shows.)
    let map = {
        let st = s.settings.lock().unwrap();
        st.screens == flight::SCREENS_CHOSEN && !st.always_on
    };
    let height = ui::SIZE[1] + if map { ui::MAP_EXTRA } else { 0.0 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Bit")
            .with_app_id("tron-bit")
            .with_inner_size([ui::SIZE[0], height])
            .with_min_inner_size([ui::SIZE[0], 300.0])
            .with_max_inner_size([ui::SIZE[0], ui::SIZE[1] + ui::MAP_EXTRA])
            .with_maximize_button(false)
            .with_icon(Arc::new(icon))
            // Elsewhere than Windows, Bit flies in a viewport of this app,
            // and eframe picks one OpenGL setup for every viewport from this
            // root one: it has to allow transparency, or Bit flies in a
            // black box. (The settings still paint an opaque background over
            // the whole window.)
            .with_transparent(cfg!(not(windows))),
        // Frames are paced by `request_repaint_after`; waiting for vsync in
        // some OpenGL drivers spins the CPU.
        vsync: false,
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Bit",
        options,
        Box::new(|cc| {
            *shared().egui.lock().unwrap() = Some(cc.egui_ctx.clone());
            Ok(Box::new(App {
                ui: ui::SettingsUi::new(&cc.egui_ctx),
            }))
        }),
    );
    *s.egui.lock().unwrap() = None;
    s.settings_open.store(false, Ordering::Relaxed);
}

#[cfg(not(target_os = "linux"))]
fn start_tray() {
    let tray = make_tray();
    if tray.is_none() {
        // No tray to reach Bit from: show the settings so it can still be quit.
        open_settings();
    }
    TRAY.with(|t| *t.borrow_mut() = tray);
}

/// On Linux the tray needs GTK's own main loop, on a thread of its own.
#[cfg(target_os = "linux")]
fn start_tray() {
    std::thread::spawn(|| {
        if gtk::init().is_err() {
            open_settings();
            return;
        }
        let tray = make_tray();
        if tray.is_none() {
            open_settings();
        }
        TRAY.with(|t| *t.borrow_mut() = tray);
        gtk::main();
    });
}
