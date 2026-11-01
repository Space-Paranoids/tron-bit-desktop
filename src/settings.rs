//! Settings, saved as `key = value` lines (the same layout as the old
//! settings.toml, so earlier settings carry over).

use std::fs::{self, File};
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub min_interval_seconds: u64,
    pub max_interval_seconds: u64,
    pub visible_seconds: u64,
    pub size_pixels: f32,
    pub sound_enabled: bool,
    pub yes_sound_enabled: bool,
    pub no_sound_enabled: bool,
    pub alarm_sound_enabled: bool,
    /// Says the long "yes, yes, yes".
    pub yes3_sound_enabled: bool,
    pub volume_percent: u8,
    pub paused: bool,
    /// No visits (and Bit stays silent) while the microphone is in use.
    pub quiet_during_calls: bool,
    /// No visits between these times (minutes after midnight, local time).
    pub quiet_hours: bool,
    pub quiet_from: u32,
    pub quiet_to: u32,
    /// Percent of visits on which Bit says something.
    pub chattiness: u8,
    /// Fly at about 30 frames a second instead of the display's rate.
    pub low_power: bool,
    /// Which screens Bit visits: all, the main one, all but the main one,
    /// or the ones chosen on the map (`flight::SCREENS_*`).
    pub screens: u8,
    /// The screens chosen on the map, by position and size (`Monitor::key`).
    pub chosen_screens: Vec<String>,
    /// Where on a screen Bit mostly hovers (`flight::SPOT_*`).
    pub spot: u8,
    /// What clicking Bit does (`CLICK_*`).
    pub click: u8,
    /// Leave Bit out of screenshots, recordings and screen sharing.
    pub hide_from_capture: bool,
    /// The key for Ask Bit, like "Ctrl+Alt+B"; empty for none.
    pub ask_key: String,
    /// Always on: Bit stays on one screen, roaming it, instead of visiting.
    pub always_on: bool,
    /// The screen it roams (`Monitor::key`; empty or gone: the main one).
    pub always_screen: String,
    /// How restless Bit is when always on, in percent.
    pub restlessness: u8,
    /// Always on: Bit hovers in one spot (Hangs out) instead of roaming.
    pub always_hover: bool,
    /// How often Bit says yes or no by itself when always on (0: never;
    /// see `chatter_seconds`).
    pub always_chatter: u8,
}

pub const CLICK_SHOO: u8 = 0;
pub const CLICK_ASK: u8 = 1;
pub const CLICK_NOTHING: u8 = 2;

impl Default for Settings {
    fn default() -> Self {
        Self {
            min_interval_seconds: 180,
            max_interval_seconds: 900,
            visible_seconds: 8,
            size_pixels: 140.0,
            sound_enabled: true,
            yes_sound_enabled: true,
            no_sound_enabled: true,
            alarm_sound_enabled: true,
            yes3_sound_enabled: true,
            volume_percent: 70,
            paused: false,
            quiet_during_calls: true,
            quiet_hours: true,
            quiet_from: 22 * 60,
            quiet_to: 8 * 60,
            chattiness: 85,
            low_power: true,
            screens: 0,
            spot: 0,
            click: CLICK_SHOO,
            hide_from_capture: false,
            chosen_screens: Vec::new(),
            ask_key: DEFAULT_ASK_KEY.to_string(),
            always_on: false,
            always_screen: String::new(),
            restlessness: 60,
            always_hover: false,
            always_chatter: 40,
        }
    }
}

pub const DEFAULT_ASK_KEY: &str = "Ctrl+Alt+B";

/// A key like "Ctrl+Alt+B" as (ctrl, alt, shift, Windows key code). Letters,
/// digits and F1 to F24; everything but a function key needs Ctrl or Alt, so
/// it can't swallow ordinary typing.
pub fn hotkey_parts(key: &str) -> Option<(bool, bool, bool, u32)> {
    let (mut ctrl, mut alt, mut shift, mut code) = (false, false, false, None);
    for part in key.split('+').map(str::trim) {
        match part {
            "Ctrl" => ctrl = true,
            "Alt" => alt = true,
            "Shift" => shift = true,
            p if p.len() == 1 && p.as_bytes()[0].is_ascii_alphanumeric() => {
                code = Some((p.as_bytes()[0].to_ascii_uppercase() as u32, false));
            }
            p => {
                let n: u32 = p.strip_prefix('F')?.parse().ok()?;
                code = (1..=24).contains(&n).then_some((0x6F + n, true));
            }
        }
    }
    let (code, function) = code?;
    (ctrl || alt || function).then_some((ctrl, alt, shift, code))
}

pub const MIN_INTERVAL: u64 = 10;
pub const MAX_INTERVAL: u64 = 4 * 3600;
pub const VISIBLE: std::ops::RangeInclusive<u64> = 4..=30;
pub const SIZE: std::ops::RangeInclusive<f32> = 60.0..=320.0;
pub const DAY: u32 = 24 * 60;

impl Settings {
    /// Pull every value back into range.
    pub fn tidy(&mut self) {
        self.min_interval_seconds = self.min_interval_seconds.clamp(MIN_INTERVAL, MAX_INTERVAL);
        self.max_interval_seconds = self
            .max_interval_seconds
            .clamp(self.min_interval_seconds, MAX_INTERVAL);
        self.visible_seconds = self.visible_seconds.clamp(*VISIBLE.start(), *VISIBLE.end());
        self.size_pixels = self.size_pixels.clamp(*SIZE.start(), *SIZE.end());
        self.volume_percent = self.volume_percent.min(100);
        self.chattiness = self.chattiness.min(100);
        self.screens = self.screens.min(3);
        self.restlessness = self.restlessness.min(100);
        self.always_chatter = self.always_chatter.min(100);
        self.spot = self.spot.min(6);
        self.click = self.click.min(CLICK_NOTHING);
        self.quiet_from %= DAY;
        self.quiet_to %= DAY;
    }

    /// The average wait (s) between yes-or-no moments when always on, for the
/// setting `v` (1 to 100: about every 10 minutes to about every 15 seconds;
/// 0: never).
pub fn chatter_seconds(v: u8) -> Option<f32> {
    (v > 0).then(|| 600.0 * (15.0f32 / 600.0).powf(f32::from(v.min(100) - 1) / 99.0))
}

/// Whether `now` (minutes after midnight) falls in quiet hours, and if so
    /// how many minutes until they end.
    pub fn quiet_left(&self, now: u32) -> Option<u32> {
        if !self.quiet_hours || self.quiet_from == self.quiet_to {
            return None;
        }
        let since = (now + DAY - self.quiet_from) % DAY;
        let span = (self.quiet_to + DAY - self.quiet_from) % DAY;
        (since < span).then(|| span - since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every setting survives being saved and read back.
    #[test]
    fn round_trip() {
        let s = Settings {
            size_pixels: 211.0,
            quiet_from: 21 * 60 + 15,
            screens: 3,
            chosen_screens: vec!["0,0,1920,1080".into(), "1920,201,1920,1080".into()],
            always_on: true,
            always_screen: "1920,201,1920,1080".into(),
            restlessness: 33,
            always_hover: true,
            always_chatter: 77,
            ask_key: "Ctrl+Shift+F9".into(),
            hide_from_capture: true,
            ..Settings::default()
        };
        assert_eq!(parse(&to_text(&s)), s);
    }

    #[test]
    fn hotkeys() {
        assert_eq!(hotkey_parts("Ctrl+Alt+B"), Some((true, true, false, 0x42)));
        assert_eq!(hotkey_parts("Alt+Shift+7"), Some((false, true, true, 0x37)));
        assert_eq!(hotkey_parts("F9"), Some((false, false, false, 0x78)));
        assert_eq!(hotkey_parts("B"), None);
        assert_eq!(hotkey_parts("Shift+B"), None);
        assert_eq!(hotkey_parts(""), None);
        assert_eq!(hotkey_parts("Ctrl+Comma"), None);
    }

    #[test]
    fn quiet_hours() {
        let s = Settings {
            quiet_hours: true,
            quiet_from: 22 * 60,
            quiet_to: 8 * 60,
            ..Settings::default()
        };
        assert_eq!(s.quiet_left(23 * 60), Some(9 * 60));
        assert_eq!(s.quiet_left(7 * 60 + 30), Some(30));
        assert_eq!(s.quiet_left(8 * 60), None);
        assert_eq!(s.quiet_left(12 * 60), None);
        let day = Settings {
            quiet_hours: true,
            quiet_from: 9 * 60,
            quiet_to: 17 * 60,
            ..Settings::default()
        };
        assert_eq!(day.quiet_left(10 * 60), Some(7 * 60));
        assert_eq!(day.quiet_left(18 * 60), None);
    }
}

pub fn config_dir() -> PathBuf {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let base = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("tron-bit")
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// No settings saved yet: Bit is starting for the first time.
pub fn first_run() -> bool {
    !settings_path().exists()
}

pub fn load() -> Settings {
    fs::read_to_string(settings_path()).map_or_else(|_| Settings::default(), |t| parse(&t))
}

/// Settings from their saved text (anything missing or unreadable keeps its
/// default).
pub fn parse(text: &str) -> Settings {
    let mut s = Settings::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        let num = || value.parse::<f64>().ok();
        let flag = || value.parse::<bool>().ok();
        match key.trim() {
            "min_interval_seconds" => {
                s.min_interval_seconds = num().map_or(s.min_interval_seconds, |v| v as u64)
            }
            "max_interval_seconds" => {
                s.max_interval_seconds = num().map_or(s.max_interval_seconds, |v| v as u64)
            }
            "visible_seconds" => s.visible_seconds = num().map_or(s.visible_seconds, |v| v as u64),
            "size_pixels" => s.size_pixels = num().map_or(s.size_pixels, |v| v as f32),
            "volume_percent" => {
                s.volume_percent = num().map_or(s.volume_percent, |v| v.clamp(0.0, 100.0) as u8)
            }
            "sound_enabled" => s.sound_enabled = flag().unwrap_or(s.sound_enabled),
            "yes_sound_enabled" => s.yes_sound_enabled = flag().unwrap_or(s.yes_sound_enabled),
            "no_sound_enabled" => s.no_sound_enabled = flag().unwrap_or(s.no_sound_enabled),
            "yes3_sound_enabled" => s.yes3_sound_enabled = flag().unwrap_or(s.yes3_sound_enabled),
            "alarm_sound_enabled" => {
                s.alarm_sound_enabled = flag().unwrap_or(s.alarm_sound_enabled)
            }
            "paused" => s.paused = flag().unwrap_or(s.paused),
            "quiet_during_calls" => s.quiet_during_calls = flag().unwrap_or(s.quiet_during_calls),
            "quiet_hours" => s.quiet_hours = flag().unwrap_or(s.quiet_hours),
            "low_power" => s.low_power = flag().unwrap_or(s.low_power),
            "screens" => s.screens = num().map_or(s.screens, |v| v as u8),
            "spot" => s.spot = num().map_or(s.spot, |v| v as u8),
            "click" => s.click = num().map_or(s.click, |v| v as u8),
            "hide_from_capture" => s.hide_from_capture = flag().unwrap_or(s.hide_from_capture),
            "ask_key" => s.ask_key = value.to_string(),
            "always_on" => s.always_on = flag().unwrap_or(s.always_on),
            "always_screen" => s.always_screen = value.to_string(),
            "chosen_screens" => {
                s.chosen_screens = value
                    .split(';')
                    .map(str::trim)
                    .filter(|k| !k.is_empty())
                    .map(String::from)
                    .collect()
            }
            "always_hover" => s.always_hover = flag().unwrap_or(s.always_hover),
            "always_chatter" => {
                s.always_chatter = num().map_or(s.always_chatter, |v| v.clamp(0.0, 100.0) as u8)
            }
            "restlessness" => {
                s.restlessness = num().map_or(s.restlessness, |v| v.clamp(0.0, 100.0) as u8)
            }
            "quiet_from" => s.quiet_from = num().map_or(s.quiet_from, |v| v as u32),
            "quiet_to" => s.quiet_to = num().map_or(s.quiet_to, |v| v as u32),
            "chattiness" => {
                s.chattiness = num().map_or(s.chattiness, |v| v.clamp(0.0, 100.0) as u8)
            }
            _ => {}
        }
    }
    s.tidy();
    s
}

pub fn save(s: &Settings) {
    let text = to_text(s);
    let path = settings_path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    // Write beside it, then swap in, so a crash never leaves half a file.
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

/// The settings as saved: `key = value` lines.
pub fn to_text(s: &Settings) -> String {
    format!(
        "# Bit desktop companion settings\n\
         min_interval_seconds = {}\n\
         max_interval_seconds = {}\n\
         visible_seconds = {}\n\
         size_pixels = {}\n\
         sound_enabled = {}\n\
         yes_sound_enabled = {}\n\
         no_sound_enabled = {}\n\
         alarm_sound_enabled = {}\n\
         yes3_sound_enabled = {}\n\
         volume_percent = {}\n\
         paused = {}\n\
         quiet_during_calls = {}\n\
         quiet_hours = {}\n\
         quiet_from = {}\n\
         quiet_to = {}\n\
         chattiness = {}\n\
         low_power = {}\n\
         screens = {}\n\
         spot = {}\n\
         click = {}\n\
         hide_from_capture = {}\n\
         chosen_screens = \"{}\"\n\
         ask_key = \"{}\"\n\
         always_on = {}\n\
         always_screen = \"{}\"\n\
         restlessness = {}\n\
         always_hover = {}\n\
         always_chatter = {}\n",
        s.min_interval_seconds,
        s.max_interval_seconds,
        s.visible_seconds,
        s.size_pixels.round(),
        s.sound_enabled,
        s.yes_sound_enabled,
        s.no_sound_enabled,
        s.alarm_sound_enabled,
        s.yes3_sound_enabled,
        s.volume_percent,
        s.paused,
        s.quiet_during_calls,
        s.quiet_hours,
        s.quiet_from,
        s.quiet_to,
        s.chattiness,
        s.low_power,
        s.screens,
        s.spot,
        s.click,
        s.hide_from_capture,
        s.chosen_screens.join(";"),
        s.ask_key,
        s.always_on,
        s.always_screen,
        s.restlessness,
        s.always_hover,
        s.always_chatter,
    )
}

/// Hold the lock this returns for the life of the program (`None` when the
/// lock file can't be made); `Err` means Bit is already running. The
/// operating system lets go of the lock however the program ends.
pub fn single_instance() -> Result<Option<File>, ()> {
    let dir = config_dir();
    let _ = fs::create_dir_all(&dir);
    let Ok(file) = File::create(dir.join("bit.lock")) else {
        return Ok(None);
    };
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(()),
        // Locks unsupported here: run rather than refuse to start.
        Err(_) => Ok(Some(file)),
    }
}
