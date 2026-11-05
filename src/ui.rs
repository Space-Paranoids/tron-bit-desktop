//! The settings window: four panels in two columns (Visits and App, Look and
//! Sound), a live Bit drawn by the same renderer as the real one, and Ask /
//! Fly in / Quit along the bottom. Every change applies at once and is saved.

use crate::bit::{self, Canvas, Mood, Pose};
use crate::flight::{Monitor, Voice, SCREENS_CHOSEN};
use crate::settings::{self, Settings};
use crate::{platform, Cmd, Shared};
use eframe::egui::{
    self, vec2, Align, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Layout, RichText, Stroke, TextStyle, TextureHandle, TextureOptions,
};
use std::sync::Arc;
use std::time::Instant;

pub const SIZE: [f32; 2] = [860.0, 694.0];
/// How much taller the window grows while the Chosen screens map shows.
pub const MAP_EXTRA: f32 = 72.0;
/// The map's height.
const MAP: f32 = 72.0;

// The teal menu palette.
/// Panel fill: the window background.
const BG: Color32 = Color32::from_rgb(0, 10, 11);
/// The raised panels, a shade up from the panel fill.
const PANEL: Color32 = Color32::from_rgb(0, 15, 17);
/// Button fill: unselected buttons, switches and slider rails.
const WIDGET: Color32 = Color32::from_rgb(0, 26, 26);
/// Title teal at reduced strength, for quieter outlines.
const LINE: Color32 = Color32::from_rgb(22, 100, 96);
/// Title teal: title lettering, panel borders, buttons.
const ACCENT: Color32 = Color32::from_rgb(56, 242, 224);
/// Bright teal: hovered and selected text and borders.
const BRIGHT: Color32 = Color32::from_rgb(115, 255, 235);
/// Highlight bar: switches that are on, the filled part of sliders.
const ACCENT_DIM: Color32 = Color32::from_rgb(26, 140, 128);
/// Pressed button.
const PRESSED: Color32 = Color32::from_rgb(26, 153, 140);
/// Menu row text, and dimmed text.
const TEXT: Color32 = Color32::from_rgb(191, 235, 230);
const MUTED: Color32 = Color32::from_rgb(140, 191, 191);
/// Red bike glow, for Quit.
const DANGER: Color32 = Color32::from_rgb(255, 26, 15);

/// Bit's preview, in points.
const PREVIEW: f32 = 92.0;

pub struct SettingsUi {
    canvas: Canvas,
    preview: Option<TextureHandle>,
    started: Instant,
    autostart: bool,
    /// Changed but not yet written to disk (saved once the mouse is let go).
    unsaved: bool,
    /// The screens, numbered left to right, as last looked up.
    monitors: Vec<Monitor>,
    monitors_at: Instant,
    /// The window's height, as last set.
    height: f32,
    /// Waiting for a new Ask Bit key to be pressed.
    capturing: bool,
}

impl SettingsUi {
    pub fn new(ctx: &egui::Context) -> Self {
        style(ctx);
        Self {
            canvas: Canvas::new(),
            preview: None,
            started: Instant::now(),
            autostart: platform::autostart(),
            unsaved: false,
            monitors: numbered(platform::monitors()),
            monitors_at: Instant::now(),
            height: SIZE[1],
            capturing: false,
        }
    }

    pub fn capturing_key(&self) -> bool {
        self.capturing
    }

    pub fn show(&mut self, ctx: &egui::Context, shared: &Shared) {
        let before = shared.settings.lock().unwrap().clone();
        let mut s = before.clone();
        self.update_preview(ctx);
        // Screens can come and go while the window is open.
        if self.monitors_at.elapsed().as_secs_f32() > 2.0 {
            self.monitors = numbered(platform::monitors());
            self.monitors_at = Instant::now();
        }
        // Room for the screen map while it shows.
        let height = if s.screens == SCREENS_CHOSEN && !s.always_on {
            SIZE[1] + MAP_EXTRA
        } else {
            SIZE[1]
        };
        if height != self.height {
            self.height = height;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(SIZE[0], height)));
        }
        let monitors = &self.monitors.clone();
        let preview = self.preview.clone();
        let preview = preview.as_ref();

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(18.0))
            .show(ctx, |ui| {
                // Scrolls if the window ends up shorter than its contents (a small
                // screen or large display scaling).
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                        self.header(ui, shared);
                        // Two columns: when and how often on the left, what it
                        // looks like, where it goes and how it sounds on the right.
                        let autostart = &mut self.autostart;
                        let capturing = &mut self.capturing;
                        ui.columns(2, |cols| {
                            cols[0].spacing_mut().item_spacing = vec2(10.0, 10.0);
                            cols[1].spacing_mut().item_spacing = vec2(10.0, 10.0);
                            panel(&mut cols[0], "VISITS", |ui| visits(ui, &mut s, monitors));
                            panel(&mut cols[0], "APP", |ui| {
                                row(ui, "Low power", |ui| {
                                    if s.always_on {
                                        // Always on is drawn non-stop: it always saves power.
                                        let mut on = true;
                                        ui.add_enabled_ui(false, |ui| toggle(ui, &mut on))
                                            .inner
                                            .on_disabled_hover_text("Always on always uses low power");
                                    } else {
                                        toggle(ui, &mut s.low_power).on_hover_text(
                                            "Bit flies at about 30 frames a second, using less CPU (good on battery)",
                                        );
                                    }
                                });
                                row(ui, "Hide from captures", |ui| {
                                    // Windows only: elsewhere it shows, switched off, with why.
                                    let on = cfg!(windows);
                                    let r = ui.add_enabled_ui(on, |ui| toggle(ui, &mut s.hide_from_capture)).inner;
                                    if on {
                                        r.on_hover_text("Bit stays on your screen but is left out of screenshots, recordings and screen sharing");
                                    } else {
                                        r.on_disabled_hover_text("Not available on this system: only Windows lets an app keep a window out of screenshots");
                                    }
                                });
                                if cfg!(windows) {
                                    row(ui, "Ask Bit key", |ui| ask_key(ui, &mut s, capturing));
                                }
                                if platform::autostart_supported() {
                                    row(ui, "Start at login", |ui| {
                                        if toggle(ui, autostart).changed() {
                                            platform::set_autostart(*autostart);
                                        }
                                    });
                                }
                            });
                            panel(&mut cols[1], "LOOK", |ui| {
                                row(ui, "Size", |ui| {
                                    ui.add(
                                        egui::Slider::new(&mut s.size_pixels, settings::SIZE)
                                            .step_by(1.0)
                                            .suffix(" px"),
                                    );
                                });
                                // Always on roams the one screen chosen under Visits.
                                let visiting = !s.always_on;
                                let off = "Always on roams the screen chosen under Visits";
                                row(ui, "Screens", |ui| {
                                    ui.add_enabled_ui(visiting, |ui| {
                                        choice(
                                            ui,
                                            "screens",
                                            &mut s.screens,
                                            &["All screens", "Main only", "All but main", "Chosen screens"],
                                        )
                                    })
                                    .inner
                                    .on_disabled_hover_text(off);
                                });
                                if s.screens == SCREENS_CHOSEN && visiting {
                                    screen_map(ui, monitors, &mut s.chosen_screens, preview);
                                }
                                row(ui, "Hangs out", |ui| {
                                    ui.add_enabled_ui(visiting || s.always_hover, |ui| {
                                        choice(
                                            ui,
                                            "spot",
                                            &mut s.spot,
                                            &["Anywhere", "Center", "Top", "Bottom", "Left", "Right", "Corners"],
                                        )
                                    })
                                    .inner
                                    .on_hover_text("Where on a screen Bit mostly hovers")
                                    .on_disabled_hover_text("Only for visits, or when Always on hovers");
                                });
                                row(ui, "Clicking Bit", |ui| {
                                    choice(ui, "click", &mut s.click, &["Shoos it away", "Asks it", "Does nothing"])
                                        .on_hover_text(
                                            "Shoo: it flies off. Ask: it stops and answers yes or no. Nothing: clicks pass through it.",
                                        );
                                });
                            });
                            panel(&mut cols[1], "SOUND", |ui| sound(ui, &mut s, shared));
                        });
                        ui.add_space(4.0);
                        footer(ui, shared);
                    });
            });

        // Just switched to chosen screens with no choice that matches these
        // screens: start from the main one.
        if s.screens == SCREENS_CHOSEN
            && before.screens != SCREENS_CHOSEN
            && !self.monitors.iter().any(|m| s.chosen_screens.contains(&m.key()))
        {
            s.chosen_screens = self
                .monitors
                .iter()
                .filter(|m| m.primary)
                .map(Monitor::key)
                .collect();
        }
        s.tidy();
        if s != before {
            let reschedule = s.paused != before.paused
                || s.min_interval_seconds != before.min_interval_seconds
                || s.max_interval_seconds != before.max_interval_seconds;
            *shared.settings.lock().unwrap() = s;
            self.unsaved = true;
            crate::settings_changed(reschedule);
        }
        if self.unsaved && !ctx.input(|i| i.pointer.any_down()) {
            settings::save(&shared.settings.lock().unwrap());
            self.unsaved = false;
        }
    }

    /// Bit hovering in the corner, changing its mind every couple of
    /// seconds: neutral, yes, neutral, no.
    fn update_preview(&mut self, ctx: &egui::Context) {
        const STEP: f32 = 2.2;
        const MOODS: [Mood; 4] = [Mood::Neutral, Mood::Yes, Mood::Neutral, Mood::No];
        let t = self.started.elapsed().as_secs_f32();
        let k = (t / STEP) as usize;
        let u = ((t - k as f32 * STEP) / bit::MORPH).min(1.0);
        let pose = Pose {
            t,
            before: MOODS[(k + 3) % 4],
            mood: MOODS[k % 4],
            u,
        };
        let side = (PREVIEW * ctx.pixels_per_point()).round() as usize;
        let c = side as f32 * 0.5;
        let bob = (t * 1.7).sin() * side as f32 * 0.03;
        let px = self
            .canvas
            .render(side, c, c + bob, c * 0.9 / bit::reach(), &pose);
        let image = egui::ColorImage {
            size: [side, side],
            pixels: px
                .iter()
                .map(|&p| {
                    Color32::from_rgba_premultiplied(
                        (p >> 16) as u8,
                        (p >> 8) as u8,
                        p as u8,
                        (p >> 24) as u8,
                    )
                })
                .collect(),
        };
        match &mut self.preview {
            Some(tex) => tex.set(image, TextureOptions::LINEAR),
            None => {
                self.preview = Some(ctx.load_texture("bit-preview", image, TextureOptions::LINEAR))
            }
        }
    }

    /// One band across the top: what Bit is and what it's up to on the
    /// left, the neon title in the middle, the live preview on the right.
    fn header(&self, ui: &mut egui::Ui, shared: &Shared) {
        let (rect, _) =
            ui.allocate_exact_size(vec2(ui.available_width(), PREVIEW), egui::Sense::hover());
        let p = ui.painter();
        let mid = rect.left() + 4.0;
        p.text(
            egui::pos2(mid, rect.center().y - 9.0),
            egui::Align2::LEFT_CENTER,
            "DESKTOP COMPANION",
            FontId::proportional(11.0),
            MUTED,
        );
        p.text(
            egui::pos2(mid, rect.center().y + 10.0),
            egui::Align2::LEFT_CENTER,
            status_text(shared),
            FontId::proportional(12.0),
            TEXT,
        );
        neon_title(p, rect.center(), "BIT", 88.0, ACCENT);
        if let Some(tex) = &self.preview {
            let r = egui::Rect::from_center_size(
                egui::pos2(rect.right() - PREVIEW * 0.5, rect.center().y),
                vec2(PREVIEW, PREVIEW),
            );
            p.image(
                tex.id(),
                r,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
}

fn visits(ui: &mut egui::Ui, s: &mut Settings, monitors: &[Monitor]) {
    row(ui, "Mode", |ui| {
        let mut mode = u8::from(s.always_on);
        choice(ui, "mode", &mut mode, &["Visits", "Always on"]).on_hover_text(
            "Visits: Bit comes by now and then. Always on: Bit stays, roaming one screen; double-click it (or use the tray menu) to go back to visits.",
        );
        s.always_on = mode == 1;
    });
    if s.always_on {
        row(ui, "On screen", |ui| {
            // The saved screen, or the main one if it's gone (as Bit does).
            let current = monitors
                .iter()
                .position(|m| m.key() == s.always_screen)
                .or_else(|| monitors.iter().position(|m| m.primary))
                .unwrap_or(0);
            egui::ComboBox::from_id_salt("always_screen")
                .width(150.0)
                .selected_text(screen_name(monitors, current))
                .show_ui(ui, |ui| {
                    for (i, m) in monitors.iter().enumerate() {
                        if ui
                            .selectable_label(i == current, screen_name(monitors, i))
                            .clicked()
                        {
                            s.always_screen = m.key();
                        }
                    }
                });
        });
        row(ui, "Movement", |ui| {
            let mut hover = u8::from(s.always_hover);
            choice(ui, "movement", &mut hover, &["Roams", "Hovers"]).on_hover_text(
                "Roams: Bit ricochets around its screen. Hovers: it stays in one spot, set by Hangs out.",
            );
            s.always_hover = hover == 1;
        });
        row(ui, "Restlessness", |ui| {
            let roaming = !s.always_hover;
            ui.add_enabled(
                roaming,
                egui::Slider::new(&mut s.restlessness, 0..=100)
                    .custom_formatter(|v, _| restless_word(v).into())
                    .custom_parser(|t| t.trim().trim_end_matches('%').parse().ok()),
            )
            .on_hover_text("How often Bit lunges at the edges of its screen, and how fast it goes")
            .on_disabled_hover_text("Only while Bit roams");
        });
        row(ui, "Says something", |ui| {
            ui.add(
                egui::Slider::new(&mut s.always_chatter, 0..=100)
                    .custom_formatter(|v, _| match Settings::chatter_seconds(v as u8) {
                        None => "Never".into(),
                        Some(secs) if secs < 60.0 => format!("~{:.0} s", (secs / 5.0).round() * 5.0),
                        Some(secs) => format!("~{:.0} min", (secs / 60.0).round()),
                    })
                    .custom_parser(|t| t.trim().parse().ok()),
            )
            .on_hover_text(
                "How often Bit says yes or no by itself, on average (as the sound settings allow). Asked something, it always answers.",
            );
        });
        row(ui, "Pause Bit", |ui| {
            toggle(ui, &mut s.paused).on_hover_text("Bit leaves until you switch this off");
        });
        quiet(ui, s);
        return;
    }
    let mut lo = s.min_interval_seconds as f32 / 60.0;
    let mut hi = s.max_interval_seconds as f32 / 60.0;
    let range = settings::MIN_INTERVAL as f32 / 60.0..=settings::MAX_INTERVAL as f32 / 60.0;
    row(ui, "Comes by after", |ui| {
        if ui.add(minutes_slider(&mut lo, range.clone())).changed() {
            s.min_interval_seconds = (lo * 60.0).round() as u64;
            s.max_interval_seconds = s.max_interval_seconds.max(s.min_interval_seconds);
        }
    });
    row(ui, "...and at most", |ui| {
        if ui.add(minutes_slider(&mut hi, range)).changed() {
            s.max_interval_seconds = (hi * 60.0).round() as u64;
            s.min_interval_seconds = s.min_interval_seconds.min(s.max_interval_seconds);
        }
    });
    row(ui, "Stays for", |ui| {
        ui.add(egui::Slider::new(&mut s.visible_seconds, settings::VISIBLE).suffix(" s"));
    });
    row(ui, "Pause visits", |ui| {
        toggle(ui, &mut s.paused);
    });
    row(ui, "Says something", |ui| {
        ui.add(egui::Slider::new(&mut s.chattiness, 0..=100).suffix(" %"))
            .on_hover_text("How often Bit says yes or no on a visit (percent of visits)");
    });
    row(ui, "On a visit", |ui| {
        let mut v = u8::from(s.changes_mind);
        choice(ui, "changes_mind", &mut v, &["One answer", "Changes its mind"]).on_hover_text(
            "One answer: yes or no, once. Changes its mind: sometimes one, a pause, then the other.",
        );
        s.changes_mind = v == 1;
    });
    quiet(ui, s);
}

/// The Restlessness setting in words.
fn restless_word(v: f64) -> &'static str {
    match v as u32 {
        0..20 => "Calm",
        20..45 => "Low",
        45..70 => "Medium",
        70..90 => "High",
        _ => "Frantic",
    }
}

/// Quiet during calls and quiet hours (in both modes).
fn quiet(ui: &mut egui::Ui, s: &mut Settings) {
    row(ui, "Quiet during calls", |ui| {
        toggle(ui, &mut s.quiet_during_calls)
            .on_hover_text("No visits while your microphone is in use; Ask Bit answers silently");
    });
    row(ui, "Quiet hours", |ui| {
        toggle(ui, &mut s.quiet_hours).on_hover_text("No visits between these times");
        ui.add_space(8.0);
        ui.add_enabled_ui(s.quiet_hours, |ui| {
            ui.add(clock(&mut s.quiet_from));
            ui.label(RichText::new("to").size(11.0).color(MUTED));
            ui.add(clock(&mut s.quiet_to));
        });
    });
}

/// The Ask Bit key: click, then press the new keys, shown as they're held.
/// Esc keeps the old one; Delete means no key.
fn ask_key(ui: &mut egui::Ui, s: &mut Settings, capturing: &mut bool) {
    let was = *capturing;
    let (mods, key) = if *capturing {
        platform::read_keys()
    } else {
        (String::new(), None)
    };
    let shown = if *capturing {
        format!("{mods}...")
    } else if s.ask_key.is_empty() {
        "None".to_string()
    } else {
        s.ask_key.clone()
    };
    let button = egui::Button::new(shown).min_size(vec2(130.0, 22.0));
    if ui
        .add(button)
        .on_hover_text("Click, then press the keys you want, with Ctrl or Alt, or a function key. Esc cancels, Delete turns it off.")
        .clicked()
    {
        *capturing = !*capturing;
    }
    if *capturing {
        ui.ctx().request_repaint();
        ui.label(RichText::new("Hold the keys").size(11.0).color(MUTED));
        match key.as_deref() {
            Some("Esc") => *capturing = false,
            Some("Delete") => {
                s.ask_key.clear();
                *capturing = false;
            }
            Some(k) => {
                let combo = format!("{mods}{k}");
                if settings::hotkey_parts(&combo).is_some() {
                    s.ask_key = combo;
                    *capturing = false;
                }
            }
            None => {}
        }
        // Clicked away: keys typed elsewhere aren't meant for this.
        if !platform::in_front() {
            *capturing = false;
        }
    } else if !s.ask_key.is_empty() && !crate::HOTKEY.load(std::sync::atomic::Ordering::Relaxed) {
        ui.label(RichText::new("In use").size(11.0).color(DANGER))
            .on_hover_text("Another program has this key: pick a different one");
    }
    if *capturing != was {
        if !*capturing {
            platform::end_keys();
        }
        crate::choosing_key(*capturing);
    }
}

/// A time of day (minutes after midnight) shown as 24-hour HH:MM: drag in
/// 15-minute steps, or click and type a time.
fn clock(minutes: &mut u32) -> egui::DragValue<'_> {
    egui::DragValue::new(minutes)
        .range(0..=settings::DAY - 15)
        .speed(0.4)
        .custom_formatter(|m, _| {
            let m = ((m / 15.0).round() * 15.0) as u32 % settings::DAY;
            format!("{:02}:{:02}", m / 60, m % 60)
        })
        .custom_parser(|text| {
            let (h, m) = text.trim().split_once(':').unwrap_or((text.trim(), "0"));
            let (h, m) = (h.trim().parse::<f64>().ok()?, m.trim().parse::<f64>().ok()?);
            (0.0..24.0)
                .contains(&h)
                .then_some(h * 60.0 + m.clamp(0.0, 59.0))
        })
}

fn minutes_slider(v: &mut f32, range: std::ops::RangeInclusive<f32>) -> egui::Slider<'_> {
    egui::Slider::new(v, range)
        .logarithmic(true)
        .custom_formatter(|m, _| {
            let secs = (m * 60.0).round();
            if secs < 60.0 {
                format!("{secs} s")
            } else if m < 60.0 {
                format!("{} min", (m * 2.0).round() / 2.0)
            } else {
                format!("{} h", (m / 6.0).round() / 10.0)
            }
        })
}

fn sound(ui: &mut egui::Ui, s: &mut Settings, shared: &Shared) {
    row(ui, "Sound", |ui| {
        toggle(ui, &mut s.sound_enabled);
    });
    ui.add_enabled_ui(s.sound_enabled, |ui| {
        row(ui, "Volume", |ui| {
            ui.add(egui::Slider::new(&mut s.volume_percent, 0..=100).suffix(" %"));
        });
        let voice = |ui: &mut egui::Ui, label: &str, on: &mut bool, v: Voice| {
            row(ui, label, |ui| {
                toggle(ui, on);
                ui.add_space(12.0);
                if ui
                    .add_enabled(*on, small_button("TRY IT"))
                    .on_hover_text("Bit flies in and says it")
                    .clicked()
                {
                    shared.send(Cmd::Visit(Some(v)));
                }
            });
        };
        voice(ui, "Says YES", &mut s.yes_sound_enabled, Voice::Yes);
        voice(ui, "Says NO", &mut s.no_sound_enabled, Voice::No);
        voice(
            ui,
            "Says YES YES YES",
            &mut s.yes3_sound_enabled,
            Voice::YesYesYes,
        );
        voice(
            ui,
            "Says NO NO NO",
            &mut s.alarm_sound_enabled,
            Voice::NoNoNo,
        );
    });
    if let Some(err) = &shared.status.lock().unwrap().audio_error {
        ui.label(
            RichText::new(format!("Sound unavailable: {err}"))
                .size(11.0)
                .color(DANGER),
        );
    }
}

fn footer(ui: &mut egui::Ui, shared: &Shared) {
    ui.horizontal(|ui| {
        let ask = egui::Button::new(RichText::new("ASK BIT").color(BG))
            .fill(ACCENT)
            .min_size(vec2(112.0, 34.0));
        let key = shared.settings.lock().unwrap().ask_key.clone();
        let hint = if crate::HOTKEY.load(std::sync::atomic::Ordering::Relaxed) {
            format!("Think of a yes/no question, then ask. ({key} works anywhere.)")
        } else {
            "Think of a yes/no question, then ask.".to_string()
        };
        if ui.add(ask).on_hover_text(hint).clicked() {
            crate::ask();
        }
        let fly = egui::Button::new("FLY IN").min_size(vec2(96.0, 34.0));
        if ui.add(fly).on_hover_text("Bit visits now").clicked() {
            shared.send(Cmd::Visit(None));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let quit = egui::Button::new(RichText::new("QUIT BIT").color(DANGER))
                .fill(PANEL)
                .stroke(Stroke::new(1.0_f32, DANGER))
                .min_size(vec2(110.0, 34.0));
            if ui.add(quit).clicked() {
                crate::quit();
            }
            ui.add_space(6.0);
            ui.label(
                RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .size(10.5)
                    .color(MUTED.gamma_multiply(0.6)),
            );
        });
    });
}

fn status_text(shared: &Shared) -> String {
    let st = shared.status.lock().unwrap();
    if st.roaming {
        return "Bit is roaming its screen".into();
    }
    if st.visiting {
        return "Bit is out and about".into();
    }
    if let Some(why) = &st.held {
        return why.clone();
    }
    match st.next_visit {
        None => "Visits are paused".into(),
        Some(at) => {
            let secs = at.saturating_duration_since(Instant::now()).as_secs();
            if secs < 60 {
                format!("Next visit in {secs} s")
            } else {
                format!("Next visit in {} min", (secs + 30) / 60)
            }
        }
    }
}

/// Title lettering lit like a neon sign: a wide soft bloom in the tube's
/// color fading out from the letters, a tight bright halo hugging the tube,
/// then the thin tube itself, nearly white-hot. Copies of the text drawn in
/// rings around it, faintest farthest out.
fn neon_title(p: &egui::Painter, center: egui::Pos2, text: &str, size: f32, color: Color32) {
    let font = FontId::new(size, FontFamily::Name("grid".into()));
    let galley = p.layout_no_wrap(text.to_owned(), font, color);
    let spread = (size / 110.0).clamp(0.5, 1.6);
    let origin = center - galley.size() * 0.5;
    let tint = |a: f32| faded(color, a);
    for (r, alpha, n) in [
        (16.0, 0.022, 24),
        (11.0, 0.03, 24),
        (7.0, 0.045, 20),
        (4.0, 0.07, 16),
        (2.0, 0.16, 12),
        (1.0, 0.35, 8),
    ] {
        let r = r * spread;
        for k in 0..n {
            let a = k as f32 / n as f32 * std::f32::consts::TAU;
            p.galley_with_override_text_color(
                origin + vec2(a.cos() * r, a.sin() * r),
                galley.clone(),
                tint(alpha),
            );
        }
    }
    p.galley_with_override_text_color(origin, galley.clone(), color);
    let hot = |c: u8| c + ((255 - c) as f32 * 0.6) as u8;
    let core = faded(
        Color32::from_rgb(hot(color.r()), hot(color.g()), hot(color.b())),
        0.85,
    );
    p.galley_with_override_text_color(origin, galley, core);
}

/// `color` at opacity `a`, premultiplied in display (gamma) space as a game
/// engine blends it. egui's own `from_rgba_unmultiplied` goes through linear
/// light, which makes faint layers far brighter and the glow a solid smear.
fn faded(color: Color32, a: f32) -> Color32 {
    let m = |c: u8| (f32::from(c) * a).round() as u8;
    Color32::from_rgba_premultiplied(
        m(color.r()),
        m(color.g()),
        m(color.b()),
        (a * 255.0).round() as u8,
    )
}

/// Text with a soft glow: four faint copies just off it, then the text.
fn glow_label(ui: &mut egui::Ui, text: &str, size: f32, color: Color32) {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), FontId::proportional(size), color);
    let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(4.0, 4.0), egui::Sense::hover());
    let origin = rect.min + vec2(2.0, 2.0);
    let off = (size / 15.0).clamp(1.0, 2.0);
    let faint = faded(color, 0.25);
    let p = ui.painter();
    for d in [
        vec2(-off, 0.0),
        vec2(off, 0.0),
        vec2(0.0, -off),
        vec2(0.0, off),
    ] {
        p.galley_with_override_text_color(origin + d, galley.clone(), faint);
    }
    p.galley(origin, galley, color);
}

/// The screens sorted left to right (then top to bottom), the order they're
/// numbered in.
fn numbered(mut monitors: Vec<Monitor>) -> Vec<Monitor> {
    monitors.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    monitors
}

fn screen_name(monitors: &[Monitor], i: usize) -> String {
    match monitors.get(i) {
        Some(m) if m.primary => format!("Screen {} (main)", i + 1),
        _ => format!("Screen {}", i + 1),
    }
}

/// The Chosen screens map: the screens laid out as they are on the desktop,
/// lit where Bit may visit. Click one to switch it (at least one stays lit).
fn screen_map(
    ui: &mut egui::Ui,
    monitors: &[Monitor],
    chosen: &mut Vec<String>,
    preview: Option<&TextureHandle>,
) {
    let (rect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), MAP), egui::Sense::hover());
    let (x0, y0, x1, y1) = monitors.iter().fold(
        (f32::MAX, f32::MAX, f32::MIN, f32::MIN),
        |(a, b, c, d), m| (a.min(m.x), b.min(m.y), c.max(m.x + m.w), d.max(m.y + m.h)),
    );
    let k = ((rect.width() - 8.0) / (x1 - x0)).min((rect.height() - 8.0) / (y1 - y0));
    let origin = rect.center() - vec2(x1 - x0, y1 - y0) * k * 0.5;
    let lit: Vec<bool> = monitors.iter().map(|m| chosen.contains(&m.key())).collect();
    let matched = lit.iter().any(|&l| l);
    for (i, m) in monitors.iter().enumerate() {
        let r = egui::Rect::from_min_size(
            origin + vec2(m.x - x0, m.y - y0) * k,
            vec2(m.w, m.h) * k,
        )
        .shrink(2.0);
        let resp = ui
            .interact(r, ui.id().with(("screen", i)), egui::Sense::click())
            .on_hover_text(if lit[i] {
                "Bit visits this screen: click to keep it off"
            } else {
                "Bit keeps off this screen: click to let it visit"
            });
        if resp.clicked() && !(lit[i] && lit.iter().filter(|&&l| l).count() == 1) {
            // Rebuild from the screens there are now, dropping any that
            // have gone.
            *chosen = monitors
                .iter()
                .zip(&lit)
                .enumerate()
                .filter(|&(j, (_, &l))| if j == i { !l } else { l })
                .map(|(_, (m, _))| m.key())
                .collect();
        }
        let p = ui.painter();
        let stroke = if lit[i] {
            Stroke::new(2.0_f32, BRIGHT)
        } else if resp.hovered() {
            Stroke::new(1.0_f32, BRIGHT)
        } else {
            Stroke::new(1.0_f32, LINE)
        };
        let fill = if lit[i] {
            Color32::from_rgb(9, 53, 50)
        } else {
            WIDGET
        };
        p.rect(r, CornerRadius::same(4), fill, stroke, egui::StrokeKind::Inside);
        let text = if lit[i] { BRIGHT } else { MUTED };
        let number_y = if m.primary { -5.0 } else { 0.0 };
        p.text(
            r.center() + vec2(0.0, number_y),
            egui::Align2::CENTER_CENTER,
            (i + 1).to_string(),
            FontId::proportional(16.0),
            text,
        );
        if m.primary {
            p.text(
                r.center() + vec2(0.0, 11.0),
                egui::Align2::CENTER_CENTER,
                "MAIN",
                FontId::proportional(8.0),
                ACCENT,
            );
        }
        if let (true, Some(tex)) = (lit[i], preview) {
            let s = (r.height() * 0.36).min(24.0);
            let b = egui::Rect::from_min_size(r.right_top() + vec2(-s - 3.0, 3.0), vec2(s, s));
            p.image(
                tex.id(),
                b,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    let note = if matched {
        "Lit: Bit visits. Dark: Bit never shows there."
    } else {
        "Your screens have changed: Bit visits all of them until you choose again."
    };
    ui.label(RichText::new(note).size(10.5).color(MUTED));
}

/// A drop-down of the given options; the value is the chosen index.
fn choice(ui: &mut egui::Ui, id: &str, value: &mut u8, options: &[&str]) -> egui::Response {
    let current = options.get(*value as usize).copied().unwrap_or(options[0]);
    egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (i, o) in options.iter().enumerate() {
                ui.selectable_value(value, i as u8, *o);
            }
        })
        .response
}

/// A titled panel spanning the window.
fn panel(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, ACCENT))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 8.0;
            glow_label(ui, title, 13.0, ACCENT);
            add(ui);
        });
}

/// A label on the left and its control in a fixed column on the right.
fn row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(132.0, 22.0), egui::Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(12.5),
            TEXT,
        );
        add(ui);
    });
}

fn small_button(text: &str) -> egui::Button<'_> {
    egui::Button::new(RichText::new(text).size(10.5)).min_size(vec2(64.0, 22.0))
}

/// An on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    if ui.is_rect_visible(rect) {
        let k = ui.ctx().animate_bool(response.id, *on);
        let enabled = ui.is_enabled();
        let fill = if *on { ACCENT_DIM } else { WIDGET };
        let stroke = if response.hovered() && enabled {
            BRIGHT
        } else {
            LINE
        };
        let p = ui.painter();
        p.rect(
            rect,
            CornerRadius::same(11),
            fill.gamma_multiply(if enabled { 1.0 } else { 0.5 }),
            Stroke::new(1.0_f32, stroke),
            egui::StrokeKind::Inside,
        );
        let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, k);
        let knob = if *on { ACCENT } else { MUTED };
        p.circle_filled(
            egui::pos2(x, rect.center().y),
            7.5,
            knob.gamma_multiply(if enabled { 1.0 } else { 0.5 }),
        );
    }
    response
}

fn style(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "orbitron".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/orbitron.ttf"
        ))),
    );
    fonts
        .families
        .insert(FontFamily::Proportional, vec!["orbitron".into()]);
    // The outlined title lettering, for the BIT title only.
    fonts.font_data.insert(
        "grid".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/grid_outline.ttf"
        ))),
    );
    fonts.families.insert(
        FontFamily::Name("grid".into()),
        vec!["grid".into(), "orbitron".into()],
    );
    fonts
        .families
        .insert(FontFamily::Monospace, vec!["orbitron".into()]);
    ctx.set_fonts(fonts);

    // One look whatever the system's light/dark setting.
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Body, FontId::proportional(12.5)),
            (TextStyle::Button, FontId::proportional(12.5)),
            (TextStyle::Small, FontId::proportional(10.0)),
            (TextStyle::Monospace, FontId::proportional(12.0)),
        ]
        .into();
        style.spacing.slider_width = 140.0;
        style.spacing.interact_size.y = 22.0;
        style.spacing.button_padding = vec2(10.0, 4.0);
        let v = &mut style.visuals;
        *v = egui::Visuals::dark();
        v.override_text_color = Some(TEXT);
        v.panel_fill = BG;
        v.window_fill = BG;
        v.extreme_bg_color = BG;
        v.faint_bg_color = PANEL;
        v.selection.bg_fill = ACCENT_DIM;
        v.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let round = CornerRadius::same(6);
        for (w, fill, stroke) in [
            (&mut v.widgets.noninteractive, PANEL, LINE),
            (&mut v.widgets.inactive, WIDGET, LINE),
            // The highlight bar at about a third strength, as on a selected menu row.
            (&mut v.widgets.hovered, Color32::from_rgb(9, 53, 50), BRIGHT),
            (&mut v.widgets.active, PRESSED, BRIGHT),
            (&mut v.widgets.open, WIDGET, ACCENT),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.bg_stroke = Stroke::new(1.0_f32, stroke);
            w.fg_stroke = Stroke::new(1.5_f32, TEXT);
            w.corner_radius = round;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    });
}
