//! Bit's voice: four WAV recordings, decoded once, levelled to a common
//! peak, and resampled with a windowed-sinc filter to the
//! output device's rate so they play back clean.
//!
//! The output device is opened only for the length of a visit and closed
//! again afterwards, so between visits Bit holds no audio stream at all
//! (an open stream keeps some systems from sleeping).

use crate::flight::Voice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Peak level the recordings are normalised to, and the level they are
/// played at.
const PEAK: f32 = 0.8;
pub const LEVEL: f32 = 0.8;

pub struct Sounds {
    rate: u32,
    yes: Vec<f32>,
    no: Vec<f32>,
    nonono: Vec<f32>,
    yesyesyes: Vec<f32>,
    /// The four, resampled for the last device rate used.
    cache: Option<(u32, [Arc<[f32]>; 4])>,
}

impl Sounds {
    pub fn load() -> Sounds {
        let one = |bytes: &[u8]| {
            decode_wav(bytes)
                .map(|(rate, v)| (rate, level(v)))
                .unwrap_or((44_100, Vec::new()))
        };
        let (rate, yes) = one(include_bytes!("../assets/sounds/bit_yes.wav"));
        let (_, no) = one(include_bytes!("../assets/sounds/bit_no.wav"));
        let (_, nonono) = one(include_bytes!("../assets/sounds/bit_nonono.wav"));
        let (_, yesyesyes) = one(include_bytes!("../assets/sounds/bit_yesyesyes.wav"));
        Sounds {
            rate,
            yes,
            no,
            nonono,
            yesyesyes,
            cache: None,
        }
    }

    fn at_rate(&mut self, rate: u32) -> [Arc<[f32]>; 4] {
        if let Some((r, bufs)) = &self.cache {
            if *r == rate {
                return bufs.clone();
            }
        }
        let bufs = [&self.yes, &self.no, &self.nonono, &self.yesyesyes]
            .map(|v| Arc::from(resample(v, self.rate, rate)));
        self.cache = Some((rate, bufs.clone()));
        bufs
    }
}

struct Playing {
    buf: Arc<[f32]>,
    pos: usize,
    gain: f32,
    /// Gain taken off per sample while fading out (0 = not fading).
    fade: f32,
}

/// An open output stream with Bit's voices ready to play on it.
pub struct Speaker {
    _stream: cpal::Stream,
    voices: Arc<Mutex<Vec<Playing>>>,
    bufs: [Arc<[f32]>; 4],
    rate: u32,
}

impl Speaker {
    pub fn open(sounds: &mut Sounds) -> Result<Speaker, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("No audio output device")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let voices: Arc<Mutex<Vec<Playing>>> = Arc::default();
        let mix = voices.clone();
        let err = |_| {};
        let stream_config = config.config();
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &stream_config,
                move |d: &mut [f32], _: &_| fill(d, channels, &mix),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_output_stream(
                &stream_config,
                move |d: &mut [i16], _: &_| fill(d, channels, &mix),
                err,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_output_stream(
                &stream_config,
                move |d: &mut [u16], _: &_| fill(d, channels, &mix),
                err,
                None,
            ),
            f => return Err(format!("Unsupported audio format {f}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Speaker {
            _stream: stream,
            voices,
            bufs: sounds.at_rate(rate),
            rate,
        })
    }

    pub fn play(&self, voice: Voice, gain: f32) {
        let buf = self.bufs[voice as usize].clone();
        if let Ok(mut v) = self.voices.lock() {
            v.push(Playing {
                buf,
                pos: 0,
                gain,
                fade: 0.0,
            });
        }
    }

    /// Fade everything out over a few milliseconds (no click).
    pub fn fade_out(&self) {
        let samples = self.rate as f32 * 0.03;
        if let Ok(mut v) = self.voices.lock() {
            for p in v.iter_mut() {
                p.fade = (p.gain / samples).max(1e-6);
            }
        }
    }

    pub fn idle(&self) -> bool {
        self.voices.lock().map_or(true, |v| v.is_empty())
    }
}

fn fill<T: cpal::SizedSample + cpal::FromSample<f32>>(
    data: &mut [T],
    channels: usize,
    voices: &Mutex<Vec<Playing>>,
) {
    let Ok(mut voices) = voices.lock() else {
        return;
    };
    for frame in data.chunks_mut(channels.max(1)) {
        let mut v = 0.0;
        for p in voices.iter_mut() {
            if let Some(s) = p.buf.get(p.pos) {
                v += s * p.gain;
                p.pos += 1;
                if p.fade > 0.0 {
                    p.gain = (p.gain - p.fade).max(0.0);
                }
            }
        }
        let s = T::from_sample(v.clamp(-1.0, 1.0));
        frame.fill(s);
    }
    voices.retain(|p| p.pos < p.buf.len() && !(p.fade > 0.0 && p.gain <= 0.0));
}

/// Scale to a common peak level, with a 3 ms fade at each end so a
/// recording never starts or stops with a click.
fn level(mut v: Vec<f32>) -> Vec<f32> {
    let peak = v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.0 {
        let k = PEAK / peak;
        let ramp = 132.min(v.len() / 2);
        let n = v.len();
        for (i, s) in v.iter_mut().enumerate() {
            let edge = i.min(n - 1 - i);
            *s *= k * if edge < ramp {
                edge as f32 / ramp as f32
            } else {
                1.0
            };
        }
    }
    v
}

/// A 16-bit PCM WAV file as mono samples, with its sample rate.
fn decode_wav(bytes: &[u8]) -> Option<(u32, Vec<f32>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let u16_at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32_at =
        |o: usize| u32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let (mut channels, mut rate, mut bits) = (0usize, 0u32, 0u16);
    let mut i = 12;
    while i + 8 <= bytes.len() {
        let size = u32_at(i + 4) as usize;
        let body = i + 8;
        match &bytes[i..i + 4] {
            b"fmt " if size >= 16 => {
                if u16_at(body) != 1 {
                    return None;
                }
                channels = u16_at(body + 2) as usize;
                rate = u32_at(body + 4);
                bits = u16_at(body + 14);
            }
            b"data" if bits == 16 && channels > 0 => {
                let end = (body + size).min(bytes.len());
                let frames = bytes[body..end]
                    .chunks_exact(2 * channels)
                    .map(|f| {
                        f.chunks_exact(2)
                            .map(|s| f32::from(i16::from_le_bytes([s[0], s[1]])))
                            .sum::<f32>()
                            / (channels as f32 * 32768.0)
                    })
                    .collect();
                return Some((rate, frames));
            }
            _ => {}
        }
        i = body + size + (size & 1);
    }
    None
}

/// Band-limited resampling (Blackman-windowed sinc, 32 zero crossings).
fn resample(src: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || src.is_empty() {
        return src.to_vec();
    }
    let ratio = f64::from(to) / f64::from(from);
    let cutoff = ratio.min(1.0) * 0.97;
    let half = 16.0 / cutoff;
    let n = (src.len() as f64 * ratio).ceil() as usize;
    let sinc = |x: f64| {
        if x.abs() < 1e-9 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        }
    };
    let window = |x: f64| {
        let k = 0.5 + 0.5 * (x / half);
        if !(0.0..=1.0).contains(&k) {
            return 0.0;
        }
        let a = 2.0 * std::f64::consts::PI * k;
        0.42 - 0.5 * a.cos() + 0.08 * (2.0 * a).cos()
    };
    (0..n)
        .map(|j| {
            let x = j as f64 / ratio;
            let lo = (x - half).ceil().max(0.0) as usize;
            let hi = ((x + half).floor() as usize).min(src.len() - 1);
            let mut acc = 0.0;
            for (i, &s) in src.iter().enumerate().take(hi + 1).skip(lo) {
                let d = x - i as f64;
                acc += f64::from(s) * cutoff * sinc(cutoff * d) * window(d);
            }
            acc as f32
        })
        .collect()
}
