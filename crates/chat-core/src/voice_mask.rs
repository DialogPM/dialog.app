//! Voice masking for voice notes ("anonymized voice"), done on the sender's device before
//! the recording is encrypted, so the original voice never leaves it.
//!
//! Two transforms, with parameters drawn at random for every recording (nothing is stored, so
//! two notes from the same person do not share a "mask" that could link them or be undone):
//! 1. McAdams formant warp (the VoicePrivacy Challenge baseline): per 20 ms frame, an LPC
//!    model of the vocal tract is computed, the angle φ of every complex pole is moved to φ^α,
//!    and the frame is resynthesized from the LPC residual. This changes the timbre (vocal
//!    tract shape) that makes a voice recognizable, while keeping the words intelligible.
//! 2. Pitch shift (WSOLA time stretch + resampling), which moves the voice's pitch up or down.
//!
//! It makes a voice hard to recognize by ear; it is not a cryptographic guarantee: the words,
//! accent and way of speaking stay, and forensic tools may still recover some speaker traits.
//!
//! Input: a WAV file (PCM 8/16/24/32-bit or 32-bit float, any channel count and sample rate).
//! Output: a 16 kHz mono 16-bit WAV.

use rand::Rng;

/// Sample rate of the processing and of the output.
pub const RATE: u32 = 16_000;

/// Masking parameters (drawn at random by [`mask_wav`]).
#[derive(Debug, Clone, Copy)]
pub struct MaskParams {
    /// McAdams coefficient: pole angles φ → φ^alpha.
    pub alpha: f64,
    /// Pitch factor (>1 higher, <1 lower).
    pub pitch: f64,
}

impl MaskParams {
    pub fn random() -> Self {
        let mut rng = rand::rng();
        let alpha = rng.random_range(0.75..0.88);
        // Up or down at random, never close to the original pitch.
        let pitch = if rng.random_bool(0.5) { rng.random_range(1.12..1.24) } else { rng.random_range(0.80..0.89) };
        MaskParams { alpha, pitch }
    }
}

/// Masks the voice in a WAV recording with random parameters; returns a 16 kHz mono WAV.
pub fn mask_wav(wav: &[u8]) -> Result<Vec<u8>, String> {
    mask_wav_with(wav, MaskParams::random())
}

pub fn mask_wav_with(wav: &[u8], params: MaskParams) -> Result<Vec<u8>, String> {
    let (samples, rate) = decode_wav(wav)?;
    if samples.is_empty() {
        return Err("empty recording".into());
    }
    let x = resample(&samples, rate, RATE);
    let y = mask_samples(&x, params);
    Ok(encode_wav(&y, RATE))
}

/// The two transforms on 16 kHz mono samples.
pub fn mask_samples(x: &[f32], params: MaskParams) -> Vec<f32> {
    let x: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    let warped = mcadams(&x, params.alpha);
    let mut y = pitch_shift(&warped, params.pitch);
    y.resize(x.len(), 0.0);
    normalize(&mut y, rms(&x));
    y.into_iter().map(|v| v as f32).collect()
}

// ---------------------------------------------------------------------------------------
// McAdams formant warp

const LPC_ORDER: usize = 20;
const FRAME: usize = 320; // 20 ms
const HOP: usize = FRAME / 2;

fn mcadams(x: &[f64], alpha: f64) -> Vec<f64> {
    let win = hann(FRAME);
    let mut out = vec![0.0; x.len() + FRAME];
    let mut start = 0;
    while start < x.len() {
        let frame: Vec<f64> = (0..FRAME).map(|i| x.get(start + i).copied().unwrap_or(0.0) * win[i]).collect();
        if let Some(a) = lpc(&frame, LPC_ORDER) {
            let residual = fir(&a, &frame);
            let warped = warp_poles(&a, alpha);
            let synth = all_pole(&warped, &residual);
            for (i, v) in synth.iter().enumerate() {
                // A broken frame (numerical trouble) is dropped rather than turned into noise.
                if v.is_finite() {
                    out[start + i] += v;
                }
            }
        }
        start += HOP;
    }
    out.truncate(x.len());
    out
}

/// Periodic Hann window (sums to 1 at 50% overlap).
fn hann(n: usize) -> Vec<f64> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect()
}

/// LPC coefficients [1, a1..ap] (autocorrelation + Levinson-Durbin); None for a silent frame.
fn lpc(frame: &[f64], order: usize) -> Option<Vec<f64>> {
    let r: Vec<f64> = (0..=order).map(|k| frame.iter().zip(&frame[k..]).map(|(a, b)| a * b).sum()).collect();
    if r[0] < 1e-9 {
        return None;
    }
    let mut a = vec![0.0; order + 1];
    a[0] = 1.0;
    // Slight lag windowing (white noise correction) keeps the recursion well conditioned.
    let mut err = r[0] * (1.0 + 1e-9);
    for i in 1..=order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / err;
        let prev = a.clone();
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            return None;
        }
    }
    Some(a)
}

/// y = A(z) x (zero initial state).
fn fir(a: &[f64], x: &[f64]) -> Vec<f64> {
    (0..x.len()).map(|n| a.iter().enumerate().take(n + 1).map(|(k, c)| c * x[n - k]).sum()).collect()
}

/// y = x / A(z) (zero initial state); a[0] = 1.
fn all_pole(a: &[f64], x: &[f64]) -> Vec<f64> {
    let mut y = vec![0.0; x.len()];
    for n in 0..x.len() {
        let mut v = x[n];
        for k in 1..a.len().min(n + 1) {
            v -= a[k] * y[n - k];
        }
        y[n] = v;
    }
    y
}

#[derive(Clone, Copy, Debug)]
struct C(f64, f64);

impl C {
    fn add(self, o: C) -> C {
        C(self.0 + o.0, self.1 + o.1)
    }
    fn sub(self, o: C) -> C {
        C(self.0 - o.0, self.1 - o.1)
    }
    fn mul(self, o: C) -> C {
        C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
    }
    fn div(self, o: C) -> C {
        let d = o.0 * o.0 + o.1 * o.1;
        C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d)
    }
    fn abs(self) -> f64 {
        self.0.hypot(self.1)
    }
    fn arg(self) -> f64 {
        self.1.atan2(self.0)
    }
    fn polar(r: f64, t: f64) -> C {
        C(r * t.cos(), r * t.sin())
    }
}

/// Roots of the monic polynomial z^p + a1 z^(p-1) + ... + ap (Durand-Kerner).
fn roots(a: &[f64]) -> Option<Vec<C>> {
    let p = a.len() - 1;
    let eval = |z: C| a.iter().fold(C(0.0, 0.0), |acc, &c| acc.mul(z).add(C(c, 0.0)));
    let mut z: Vec<C> = (0..p).map(|k| C::polar(0.9, 2.0 * std::f64::consts::PI * (k as f64 + 0.25) / p as f64)).collect();
    for _ in 0..500 {
        let mut moved = 0.0f64;
        for i in 0..p {
            let mut den = C(1.0, 0.0);
            for j in 0..p {
                if i != j {
                    den = den.mul(z[i].sub(z[j]));
                }
            }
            if den.abs() < 1e-300 {
                den = C(1e-12, 0.0);
            }
            let step = eval(z[i]).div(den);
            z[i] = z[i].sub(step);
            moved = moved.max(step.abs());
        }
        if moved < 1e-12 {
            break;
        }
    }
    z.iter().all(|r| r.0.is_finite() && r.1.is_finite()).then_some(z)
}

/// Moves the angle of every complex pole of 1/A(z) to φ^alpha (sign kept, so conjugate pairs
/// stay conjugate); real poles and the radii are unchanged, so the filter stays stable.
fn warp_poles(a: &[f64], alpha: f64) -> Vec<f64> {
    let Some(rs) = roots(a) else { return a.to_vec() };
    let warped: Vec<C> = rs
        .into_iter()
        .map(|r| {
            let t = r.arg();
            if r.1.abs() < 1e-9 {
                // Real pole (inside the unit circle by construction: keep it there).
                C(r.0.clamp(-0.999, 0.999), 0.0)
            } else {
                C::polar(r.abs().min(0.999), t.signum() * t.abs().powf(alpha))
            }
        })
        .collect();
    // Back to coefficients: prod (z - r).
    let mut poly = vec![C(1.0, 0.0)];
    for r in warped {
        let mut next = vec![C(0.0, 0.0); poly.len() + 1];
        for (i, c) in poly.iter().enumerate() {
            next[i] = next[i].add(*c);
            next[i + 1] = next[i + 1].sub(c.mul(r));
        }
        poly = next;
    }
    poly.into_iter().map(|c| c.0).collect()
}

// ---------------------------------------------------------------------------------------
// Pitch shift: WSOLA time stretch by `factor`, then resampling back to the original length.

const W: usize = 512; // 32 ms
const HS: usize = W / 2;
const TOL: usize = 160;

fn pitch_shift(x: &[f64], factor: f64) -> Vec<f64> {
    if (factor - 1.0).abs() < 1e-3 {
        return x.to_vec();
    }
    let stretched = wsola(x, factor);
    // Reading the stretched signal `factor` times faster: higher pitch for factor > 1. Low-pass
    // first when that compresses the spectrum upwards, so nothing folds back (aliasing).
    let src = if factor > 1.0 { lowpass(&stretched, 0.5 / factor) } else { stretched };
    let n = (src.len() as f64 / factor) as usize;
    (0..n).map(|i| interp(&src, i as f64 * factor)).collect()
}

fn wsola(x: &[f64], stretch: f64) -> Vec<f64> {
    let win = hann(W);
    let ha = HS as f64 / stretch;
    let out_len = (x.len() as f64 * stretch) as usize + W;
    let mut out = vec![0.0; out_len + W];
    let at = |i: isize| if i >= 0 { x.get(i as usize).copied().unwrap_or(0.0) } else { 0.0 };
    // Where the previous segment would naturally have continued.
    let mut prev_natural: isize = HS as isize;
    let mut k = 0usize;
    loop {
        let out_pos = k * HS;
        if out_pos >= out_len {
            break;
        }
        let nominal = (k as f64 * ha) as isize;
        let delta = if k == 0 {
            0
        } else {
            // The offset whose segment looks most like the natural continuation.
            let mut best = (f64::MIN, 0isize);
            let tol = TOL as isize;
            let mut d = -tol;
            while d <= tol {
                let mut c = 0.0;
                for i in (0..W).step_by(2) {
                    c += at(nominal + d + i as isize) * at(prev_natural + i as isize);
                }
                if c > best.0 {
                    best = (c, d);
                }
                d += 2;
            }
            best.1
        };
        let start = nominal + delta;
        for i in 0..W {
            out[out_pos + i] += at(start + i as isize) * win[i];
        }
        prev_natural = start + HS as isize;
        k += 1;
    }
    out.truncate(out_len - W);
    out
}

/// Windowed-sinc low-pass; `cutoff` as a fraction of the sample rate (< 0.5).
fn lowpass(x: &[f64], cutoff: f64) -> Vec<f64> {
    const TAPS: isize = 32;
    let h: Vec<f64> = (-TAPS..=TAPS)
        .map(|n| {
            let n = n as f64;
            let sinc = if n == 0.0 { 2.0 * cutoff } else { (2.0 * std::f64::consts::PI * cutoff * n).sin() / (std::f64::consts::PI * n) };
            let w = 0.54 + 0.46 * (std::f64::consts::PI * n / TAPS as f64).cos();
            sinc * w
        })
        .collect();
    (0..x.len() as isize)
        .map(|i| {
            h.iter()
                .enumerate()
                .map(|(k, c)| {
                    let j = i + k as isize - TAPS;
                    if j >= 0 && (j as usize) < x.len() { c * x[j as usize] } else { 0.0 }
                })
                .sum()
        })
        .collect()
}

fn interp(x: &[f64], pos: f64) -> f64 {
    let i = pos.floor() as usize;
    let f = pos - i as f64;
    let a = x.get(i).copied().unwrap_or(0.0);
    let b = x.get(i + 1).copied().unwrap_or(0.0);
    a + (b - a) * f
}

fn rms(x: &[f64]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| v * v).sum::<f64>() / x.len() as f64).sqrt()
}

/// Brings the loudness back to the original's, without clipping.
fn normalize(y: &mut [f64], target_rms: f64) {
    let r = rms(y);
    let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    if r < 1e-9 || peak < 1e-9 {
        return;
    }
    let gain = (target_rms / r).min(0.95 / peak);
    y.iter_mut().for_each(|v| *v *= gain);
}

// ---------------------------------------------------------------------------------------
// WAV in / out

fn decode_wav(b: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let bad = || "not a supported WAV file".to_string();
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err(bad());
    }
    let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pos = 12;
    while pos + 8 <= b.len() {
        let id = &b[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;
        if id == b"fmt " && body + 16 <= b.len() {
            let mut format = u16_at(body);
            if format == 0xFFFE && body + 26 <= b.len() {
                format = u16_at(body + 24); // WAVE_FORMAT_EXTENSIBLE: the sub-format's first field
            }
            fmt = Some((format, u16_at(body + 2), u32_at(body + 4), u16_at(body + 14)));
        } else if id == b"data" {
            let (format, channels, rate, bits) = fmt.ok_or_else(bad)?;
            // Recorders that could not seek back write 0 or a too-large size: use what is there.
            let end = if size == 0 || body + size > b.len() { b.len() } else { body + size };
            let data = &b[body..end];
            let channels = channels.max(1) as usize;
            let width = (bits as usize).div_ceil(8);
            if width == 0 || rate == 0 {
                return Err(bad());
            }
            let sample = |s: &[u8]| -> Option<f32> {
                Some(match (format, bits) {
                    (1, 8) => (s[0] as f32 - 128.0) / 128.0,
                    (1, 16) => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
                    (1, 24) => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
                    (1, 32) => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
                    (3, 32) => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
                    _ => return None,
                })
            };
            let mut out = Vec::with_capacity(data.len() / (width * channels));
            for frame in data.chunks_exact(width * channels) {
                let mut sum = 0.0;
                for c in frame.chunks_exact(width) {
                    sum += sample(c).ok_or_else(bad)?;
                }
                out.push(sum / channels as f32);
            }
            return Ok((out, rate));
        }
        pos = body + size + (size & 1);
    }
    Err(bad())
}

/// Linear-interpolation resampling, low-passed first when lowering the rate.
fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let x64: Vec<f64> = x.iter().map(|&v| v as f64).collect();
    let src = if from > to { lowpass(&x64, 0.5 * to as f64 / from as f64 * 0.95) } else { x64 };
    let step = from as f64 / to as f64;
    let n = (x.len() as f64 / step) as usize;
    (0..n).map(|i| interp(&src, i as f64 * step) as f32).collect()
}

pub fn encode_wav(y: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (y.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + y.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for v in y {
        out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A vowel-like test signal: 120 Hz pulse train through two formant resonators.
    fn vowel(seconds: f64, rate: u32) -> Vec<f32> {
        let n = (seconds * rate as f64) as usize;
        let period = (rate as f64 / 120.0) as usize;
        let src: Vec<f64> = (0..n).map(|i| if i % period == 0 { 1.0 } else { 0.0 }).collect();
        let mut y = src;
        for (f, bw) in [(700.0, 80.0), (1200.0, 90.0)] {
            let r = (-std::f64::consts::PI * bw / rate as f64).exp();
            let t = 2.0 * std::f64::consts::PI * f / rate as f64;
            let a = [1.0, -2.0 * r * t.cos(), r * r];
            y = all_pole(&a, &y);
        }
        let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        y.iter().map(|v| (v / peak * 0.5) as f32).collect()
    }

    /// Dominant period by autocorrelation (60..400 Hz), in samples.
    fn period(x: &[f32], rate: u32) -> usize {
        let (lo, hi) = ((rate / 400) as usize, (rate / 60) as usize);
        (lo..hi)
            .max_by(|&a, &b| {
                let c = |l: usize| x.iter().zip(&x[l..]).map(|(p, q)| (p * q) as f64).sum::<f64>();
                c(a).partial_cmp(&c(b)).unwrap()
            })
            .unwrap()
    }

    #[test]
    fn wav_roundtrip_and_formats() {
        let x = vowel(0.5, 48_000);
        let wav = encode_wav(&x, 48_000);
        let (y, rate) = decode_wav(&wav).unwrap();
        assert_eq!(rate, 48_000);
        assert_eq!(y.len(), x.len());
        assert!(decode_wav(b"RIFF....WAVEjunk").is_err());
        assert!(decode_wav(b"hello").is_err());
    }

    #[test]
    fn keeps_length_and_loudness_and_shifts_pitch() {
        let x = vowel(1.0, RATE);
        for pitch in [0.85, 1.2] {
            let y = mask_samples(&x, MaskParams { alpha: 0.8, pitch });
            assert_eq!(y.len(), x.len());
            assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
            let (rx, ry) = (rms(&x.iter().map(|&v| v as f64).collect::<Vec<_>>()), rms(&y.iter().map(|&v| v as f64).collect::<Vec<_>>()));
            assert!(ry > rx * 0.3 && ry < rx * 1.2, "loudness {rx} -> {ry}");
            let (px, py) = (period(&x[2000..14000], RATE) as f64, period(&y[2000..14000], RATE) as f64);
            let measured = px / py;
            assert!((measured - pitch).abs() < 0.06, "pitch factor {pitch}: measured {measured}");
        }
    }

    #[test]
    fn output_differs_and_parameters_are_random() {
        let wav = encode_wav(&vowel(0.6, 44_100), 44_100);
        let a = mask_wav(&wav).unwrap();
        let b = mask_wav(&wav).unwrap();
        let (ya, ra) = decode_wav(&a).unwrap();
        assert_eq!(ra, RATE);
        assert!(ya.len().abs_diff((0.6 * RATE as f64) as usize) <= 2);
        assert_ne!(a, b, "two maskings of the same note must differ");
        let p = MaskParams::random();
        assert!((0.75..0.88).contains(&p.alpha));
        assert!((0.80..0.89).contains(&p.pitch) || (1.12..1.24).contains(&p.pitch));
    }

    #[test]
    fn silence_stays_silent() {
        let y = mask_samples(&vec![0.0; 8000], MaskParams { alpha: 0.8, pitch: 1.2 });
        assert!(y.iter().all(|v| *v == 0.0));
    }
}
