//! Real-time voice effects for calls (and voice notes), processed on the speaker's device so
//! the other side only ever receives the changed voice.
//!
//! [`Processor`] works sample by sample at the audio device's rate, with no allocation after
//! [`Processor::new`], so it can run on an audio thread (the desktop app runs it as WebAssembly
//! in an AudioWorklet: app/src/lib/voiceFx.ts). Effects:
//! - `ANONYMOUS`: McAdams formant warp + pitch shift with random parameters per processor (per
//!   call), the live version of chat-core's voice_mask.rs; processed at 16 kHz.
//! - `DEEP` / `CHIPMUNK`: pitch (and formants) down / up.
//! - `ROBOT`: ring modulation and a short metallic comb.
//! - `HALL`: reverb (Schroeder: parallel combs and series all-pass filters).
//!
//! - noise reduction (independent of the effect, applied first): RNNoise (nnnoiseless).
//!
//! Latency: about 40 ms for the pitch effects, 60 ms for anonymous, none for robot and hall,
//! plus 10 ms with noise reduction.

pub mod lpc;

#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

use std::f32::consts::PI;

pub const NONE: u32 = 0;
pub const ANONYMOUS: u32 = 1;
pub const DEEP: u32 = 2;
pub const CHIPMUNK: u32 = 3;
pub const ROBOT: u32 = 4;
pub const HALL: u32 = 5;
pub const EFFECT_COUNT: u32 = 6;

const DEEP_RATIO: f32 = 0.74;
const CHIPMUNK_RATIO: f32 = 1.55;

/// Small deterministic generator for the anonymous effect's parameters.
struct XorShift(u32);

impl XorShift {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Parameters of the anonymous effect drawn from `seed`: (McAdams alpha, pitch ratio).
pub fn anonymous_params(seed: u32) -> (f64, f32) {
    let mut rng = XorShift(seed.max(1));
    let alpha = rng.range(0.75, 0.88) as f64;
    let pitch = if rng.next() < 0.5 { rng.range(1.12, 1.22) } else { rng.range(0.82, 0.9) };
    (alpha, pitch)
}

pub struct Processor {
    rate: f32,
    effect: u32,
    /// Noise reduction before the effect (created when first turned on).
    denoise: Option<Denoise>,
    denoise_on: bool,
    pitch: PitchShift,
    anon: Anonymous,
    robot: Robot,
    hall: Hall,
}

impl Processor {
    /// `rate`: the audio device's sample rate; `seed`: random seed of the anonymous effect.
    pub fn new(rate: f32, seed: u32) -> Self {
        let rate = rate.clamp(8000.0, 192_000.0);
        Processor {
            rate,
            effect: NONE,
            denoise: None,
            denoise_on: false,
            pitch: PitchShift::new(rate, 1.0),
            anon: Anonymous::new(rate, seed),
            robot: Robot::new(rate),
            hall: Hall::new(rate),
        }
    }

    pub fn effect(&self) -> u32 {
        self.effect
    }

    /// Switches the effect (unknown values mean none); the effect's state starts afresh.
    pub fn set_effect(&mut self, effect: u32) {
        let effect = if effect < EFFECT_COUNT { effect } else { NONE };
        if effect == self.effect {
            return;
        }
        self.effect = effect;
        match effect {
            DEEP => self.pitch = PitchShift::new(self.rate, DEEP_RATIO),
            CHIPMUNK => self.pitch = PitchShift::new(self.rate, CHIPMUNK_RATIO),
            ANONYMOUS => self.anon.reset(),
            ROBOT => self.robot.reset(),
            HALL => self.hall.reset(),
            _ => {}
        }
    }

    pub fn denoise(&self) -> bool {
        self.denoise_on
    }

    /// Turns the noise reduction on or off (applied before the effect).
    pub fn set_denoise(&mut self, on: bool) {
        if on && self.denoise.is_none() {
            self.denoise = Some(Denoise::new(self.rate));
        }
        self.denoise_on = on;
    }

    /// Processes `buf` in place.
    pub fn process(&mut self, buf: &mut [f32]) {
        if self.denoise_on {
            if let Some(d) = self.denoise.as_mut() {
                buf.iter_mut().for_each(|v| *v = d.tick(*v));
            }
        }
        match self.effect {
            DEEP | CHIPMUNK => buf.iter_mut().for_each(|v| *v = soft_clip(self.pitch.tick(*v))),
            ANONYMOUS => buf.iter_mut().for_each(|v| *v = soft_clip(self.anon.tick(*v))),
            ROBOT => buf.iter_mut().for_each(|v| *v = soft_clip(self.robot.tick(*v))),
            HALL => buf.iter_mut().for_each(|v| *v = soft_clip(self.hall.tick(*v))),
            _ => {}
        }
    }
}

/// Keeps peaks below full scale without hard clipping (linear up to 0.8).
fn soft_clip(x: f32) -> f32 {
    if !x.is_finite() {
        return 0.0;
    }
    let a = x.abs();
    if a <= 0.8 {
        x
    } else {
        x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh())
    }
}

// ---------------------------------------------------------------------------------------

/// Second-order low-pass filter (RBJ biquad).
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(cutoff: f32, rate: f32, q: f32) -> Self {
        let w = 2.0 * PI * (cutoff / rate).min(0.49);
        let (s, c) = w.sin_cos();
        let alpha = s / (2.0 * q);
        let a0 = 1.0 + alpha;
        Biquad {
            b0: (1.0 - c) / 2.0 / a0,
            b1: (1.0 - c) / a0,
            b2: (1.0 - c) / 2.0 / a0,
            a1: -2.0 * c / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 4th-order Butterworth low-pass (two biquads).
#[derive(Clone, Copy)]
struct Lowpass4([Biquad; 2]);

impl Lowpass4 {
    fn new(cutoff: f32, rate: f32) -> Self {
        Lowpass4([Biquad::lowpass(cutoff, rate, 0.541), Biquad::lowpass(cutoff, rate, 1.307)])
    }
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.0[0].tick(x);
        self.0[1].tick(y)
    }
}

/// Delay-line pitch shifter: two read taps sweep through a short window, crossfaded so one is
/// always away from the jump. Shifts pitch and formants together (like playing faster/slower).
struct PitchShift {
    buf: Vec<f32>,
    mask: usize,
    w: usize,
    phase: f32,
    window: f32,
    ratio: f32,
}

impl PitchShift {
    fn new(rate: f32, ratio: f32) -> Self {
        let window = (rate * 0.04).max(64.0);
        let size = ((window as usize + 8) * 2).next_power_of_two();
        PitchShift { buf: vec![0.0; size], mask: size - 1, w: 0, phase: 0.0, window, ratio }
    }
    fn read(&self, delay: f32) -> f32 {
        // At least 2 samples behind the write position, so interpolation never reads ahead.
        let pos = self.w as f32 - delay - 2.0 + self.buf.len() as f32;
        let i = pos.floor();
        let f = pos - i;
        let i = i as usize;
        let a = self.buf[i & self.mask];
        let b = self.buf[(i + 1) & self.mask];
        a + (b - a) * f
    }
    fn tick(&mut self, x: f32) -> f32 {
        self.buf[self.w] = x;
        self.phase = (self.phase + (1.0 - self.ratio) / self.window).rem_euclid(1.0);
        let p2 = (self.phase + 0.5).rem_euclid(1.0);
        let g1 = (PI * self.phase).sin().powi(2);
        let g2 = (PI * p2).sin().powi(2);
        let y = self.read(self.phase * self.window) * g1 + self.read(p2 * self.window) * g2;
        self.w = (self.w + 1) & self.mask;
        y
    }
}

/// Fixed-capacity FIFO of samples.
struct Fifo<const N: usize> {
    buf: [f64; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Fifo<N> {
    fn new() -> Self {
        Fifo { buf: [0.0; N], head: 0, len: 0 }
    }
    fn push(&mut self, v: f64) {
        if self.len == N {
            // Full (should not happen): drop the oldest sample.
            self.head = (self.head + 1) % N;
            self.len -= 1;
        }
        self.buf[(self.head + self.len) % N] = v;
        self.len += 1;
    }
    fn pop(&mut self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        let v = self.buf[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        v
    }
}

/// Streaming McAdams formant warp at 16 kHz (20 ms frames, 10 ms hop, Hann overlap-add).
struct Mcadams {
    alpha: f64,
    hist: [f64; MC_FRAME],
    pending: [f64; MC_HOP],
    npending: usize,
    ola: [f64; MC_FRAME],
    out: Fifo<{ 4 * MC_HOP }>,
    warp: lpc::PoleWarp,
}

const MC_RATE: f32 = 16_000.0;
const MC_FRAME: usize = 320;
const MC_HOP: usize = MC_FRAME / 2;
const MC_ORDER: usize = 20;

impl Mcadams {
    fn new(alpha: f64) -> Self {
        let mut m = Mcadams {
            alpha,
            hist: [0.0; MC_FRAME],
            pending: [0.0; MC_HOP],
            npending: 0,
            ola: [0.0; MC_FRAME],
            out: Fifo::new(),
            warp: lpc::PoleWarp::default(),
        };
        m.prime();
        m
    }
    fn prime(&mut self) {
        // One hop of silence first, so a sample can be returned for every one pushed.
        for _ in 0..MC_HOP {
            self.out.push(0.0);
        }
    }
    fn tick(&mut self, x: f64) -> f64 {
        self.pending[self.npending] = x;
        self.npending += 1;
        if self.npending == MC_HOP {
            self.npending = 0;
            self.hist.copy_within(MC_HOP.., 0);
            self.hist[MC_FRAME - MC_HOP..].copy_from_slice(&self.pending);
            self.frame();
        }
        self.out.pop()
    }
    fn frame(&mut self) {
        let mut win = [0.0f64; MC_FRAME];
        for (i, w) in win.iter_mut().enumerate() {
            *w = self.hist[i] * lpc::hann(i, MC_FRAME);
        }
        let mut a = [0.0f64; MC_ORDER + 1];
        let mut synth = [0.0f64; MC_FRAME];
        if lpc::lpc(&win, &mut a) {
            let mut residual = [0.0f64; MC_FRAME];
            lpc::fir(&a, &win, &mut residual);
            let mut warped = [0.0f64; MC_ORDER + 1];
            self.warp.warp(&a, self.alpha, &mut warped);
            lpc::all_pole(&warped, &residual, &mut synth);
            if synth.iter().any(|v| !v.is_finite()) {
                synth = [0.0; MC_FRAME];
            }
        }
        for (o, s) in self.ola.iter_mut().zip(&synth) {
            *o += s;
        }
        for i in 0..MC_HOP {
            self.out.push(self.ola[i]);
        }
        self.ola.copy_within(MC_HOP.., 0);
        self.ola[MC_FRAME - MC_HOP..].iter_mut().for_each(|v| *v = 0.0);
    }
}

/// Runs a per-sample stage at another sample rate: device rate → `inner` rate (low-passed,
/// linear interpolation) → stage → back to the device rate, a few inner samples behind.
struct RateBridge {
    rate: f32,
    inner: f32,
    down_lp: Lowpass4,
    up_lp: Lowpass4,
    /// Input position (in device samples) of the next inner sample.
    next_in: f64,
    prev_in: f32,
    n_in: u64,
    /// Processed inner samples (ring) and how many were produced.
    low: [f32; 64],
    produced: u64,
    /// Read position, in inner samples.
    read: f64,
}

impl RateBridge {
    fn new(rate: f32, inner: f32) -> Self {
        let cutoff = (0.45 * rate.min(inner)).min(20_000.0);
        RateBridge {
            rate,
            inner,
            down_lp: Lowpass4::new(cutoff, rate),
            up_lp: Lowpass4::new(cutoff, rate),
            next_in: 0.0,
            prev_in: 0.0,
            n_in: 0,
            low: [0.0; 64],
            produced: 0,
            read: -4.0,
        }
    }

    fn tick(&mut self, x: f32, stage: &mut impl FnMut(f32) -> f32) -> f32 {
        let step_in = self.rate as f64 / self.inner as f64;
        // Every inner-rate instant between the previous and this input sample.
        let lp = self.down_lp.tick(x);
        let n = self.n_in as f64;
        while self.next_in <= n {
            let frac = (self.next_in - (n - 1.0)) as f32;
            let v = self.prev_in + (lp - self.prev_in) * frac.clamp(0.0, 1.0);
            self.low[(self.produced % 64) as usize] = stage(v);
            self.produced += 1;
            self.next_in += step_in;
        }
        self.prev_in = lp;
        self.n_in += 1;
        self.read += 1.0 / step_in;
        let i = self.read.floor();
        let produced = self.produced;
        let low = &self.low;
        let at = |k: f64| if k < 0.0 || k as u64 >= produced || produced - k as u64 > 60 { 0.0 } else { low[(k as u64 % 64) as usize] };
        let y = at(i) + (at(i + 1.0) - at(i)) * (self.read - i) as f32;
        self.up_lp.tick(y)
    }
}

/// Anonymous: McAdams formant warp then pitch shift, at 16 kHz, with the loudness brought back
/// to the input's.
struct Anonymous {
    rate: f32,
    seed: u32,
    bridge: RateBridge,
    mc: Mcadams,
    pitch: PitchShift,
    env_in: f32,
    env_out: f32,
}

impl Anonymous {
    fn new(rate: f32, seed: u32) -> Self {
        let (alpha, ratio) = anonymous_params(seed);
        Anonymous {
            rate,
            seed,
            bridge: RateBridge::new(rate, MC_RATE),
            mc: Mcadams::new(alpha),
            pitch: PitchShift::new(MC_RATE, ratio),
            env_in: 0.0,
            env_out: 0.0,
        }
    }
    fn reset(&mut self) {
        *self = Anonymous::new(self.rate, self.seed);
    }
    fn tick(&mut self, x: f32) -> f32 {
        let (mc, pitch) = (&mut self.mc, &mut self.pitch);
        let y = self.bridge.tick(x, &mut |v| pitch.tick(mc.tick(v as f64) as f32));
        // Loudness: follow the input's level (300 ms envelopes), gain limited to 0.25..4.
        let k = 1.0 / (0.3 * self.rate);
        self.env_in += (x * x - self.env_in) * k;
        self.env_out += (y * y - self.env_out) * k;
        let gain = if self.env_out > 1e-10 { (self.env_in / self.env_out).sqrt().clamp(0.25, 4.0) } else { 1.0 };
        y * gain
    }
}

/// Noise reduction: RNNoise (a small recurrent network trained to keep speech and drop
/// background noise), on 10 ms frames at 48 kHz.
struct Denoise {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    frame: [f32; nnnoiseless::FRAME_SIZE],
    filled: usize,
    out: [f32; nnnoiseless::FRAME_SIZE],
    /// Processed frame being returned; starts as one frame of silence (10 ms latency).
    pos: usize,
    /// Device rate is not 48 kHz: frames go through a rate bridge.
    bridge: Option<RateBridge>,
}

const DENOISE_RATE: f32 = 48_000.0;

impl Denoise {
    fn new(rate: f32) -> Self {
        Denoise {
            state: nnnoiseless::DenoiseState::new(),
            frame: [0.0; nnnoiseless::FRAME_SIZE],
            filled: 0,
            out: [0.0; nnnoiseless::FRAME_SIZE],
            pos: 0,
            bridge: ((rate - DENOISE_RATE).abs() > 1.0).then(|| RateBridge::new(rate, DENOISE_RATE)),
        }
    }

    /// One 48 kHz sample in, one out (a frame behind).
    fn tick48(&mut self, x: f32) -> f32 {
        // RNNoise works on 16-bit sample values.
        self.frame[self.filled] = x * 32768.0;
        self.filled += 1;
        let y = self.out[self.pos] / 32768.0;
        self.pos += 1;
        if self.filled == nnnoiseless::FRAME_SIZE {
            self.state.process_frame(&mut self.out, &self.frame);
            self.filled = 0;
            self.pos = 0;
        }
        y
    }

    fn tick(&mut self, x: f32) -> f32 {
        match self.bridge.take() {
            None => self.tick48(x),
            Some(mut b) => {
                let y = b.tick(x, &mut |v| self.tick48(v));
                self.bridge = Some(b);
                y
            }
        }
    }
}

/// Robot: ring modulation (the "dalek" voice) plus a short feedback comb for a metallic ring.
struct Robot {
    rate: f32,
    phase: f32,
    comb: Vec<f32>,
    pos: usize,
}

impl Robot {
    fn new(rate: f32) -> Self {
        Robot { rate, phase: 0.0, comb: vec![0.0; ((rate * 0.006) as usize).max(1)], pos: 0 }
    }
    fn reset(&mut self) {
        self.phase = 0.0;
        self.comb.iter_mut().for_each(|v| *v = 0.0);
    }
    fn tick(&mut self, x: f32) -> f32 {
        self.phase = (self.phase + 55.0 / self.rate).fract();
        let rm = x * (2.0 * PI * self.phase).sin() * 1.6;
        let y = rm + 0.55 * self.comb[self.pos];
        self.comb[self.pos] = y;
        self.pos = (self.pos + 1) % self.comb.len();
        y * 0.7
    }
}

/// Hall reverb: 4 parallel feedback combs (with damping) and 2 series all-pass filters.
struct Hall {
    combs: [Delay; 4],
    damp: [f32; 4],
    allpass: [Delay; 2],
}

struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(len: usize) -> Self {
        Delay { buf: vec![0.0; len.max(1)], pos: 0 }
    }
    fn out(&self) -> f32 {
        self.buf[self.pos]
    }
    fn write(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    fn reset(&mut self) {
        self.buf.iter_mut().for_each(|v| *v = 0.0);
    }
}

impl Hall {
    fn new(rate: f32) -> Self {
        let ms = |m: f32| (rate * m / 1000.0) as usize;
        Hall {
            combs: [Delay::new(ms(29.7)), Delay::new(ms(37.1)), Delay::new(ms(41.1)), Delay::new(ms(43.7))],
            damp: [0.0; 4],
            allpass: [Delay::new(ms(5.0)), Delay::new(ms(1.7))],
        }
    }
    fn reset(&mut self) {
        self.combs.iter_mut().for_each(Delay::reset);
        self.allpass.iter_mut().for_each(Delay::reset);
        self.damp = [0.0; 4];
    }
    fn tick(&mut self, x: f32) -> f32 {
        let mut wet = 0.0;
        for (c, d) in self.combs.iter_mut().zip(self.damp.iter_mut()) {
            let o = c.out();
            *d = o * 0.6 + *d * 0.4;
            c.write(x + *d * 0.8);
            wet += o;
        }
        let mut y = wet * 0.25;
        for ap in &mut self.allpass {
            let o = ap.out();
            ap.write(y + o * 0.6);
            y = o - y * 0.6;
        }
        x * 0.85 + y * 0.45
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: f32, hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| 0.4 * (2.0 * PI * hz * i as f32 / rate).sin()).collect()
    }

    /// Dominant frequency by zero crossings.
    fn freq(x: &[f32], rate: f32) -> f32 {
        let crossings = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        crossings as f32 * rate / x.len() as f32
    }

    fn run(effect: u32, rate: f32, input: &[f32]) -> Vec<f32> {
        let mut p = Processor::new(rate, 1234);
        p.set_effect(effect);
        let mut out = input.to_vec();
        for block in out.chunks_mut(128) {
            p.process(block);
        }
        out
    }

    #[test]
    fn none_is_bit_exact() {
        let x = tone(48_000.0, 220.0, 4800);
        assert_eq!(run(NONE, 48_000.0, &x), x);
    }

    #[test]
    fn every_effect_is_finite_and_bounded() {
        for rate in [44_100.0, 48_000.0] {
            let x = tone(rate, 200.0, rate as usize);
            for e in 1..EFFECT_COUNT {
                let y = run(e, rate, &x);
                assert_eq!(y.len(), x.len());
                assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 1.0), "effect {e} at {rate}");
                let energy: f32 = y[y.len() / 2..].iter().map(|v| v * v).sum();
                assert!(energy > 1.0, "effect {e} at {rate} is silent ({energy})");
            }
        }
    }

    #[test]
    fn pitch_effects_shift_the_pitch() {
        let rate = 48_000.0;
        let x = tone(rate, 200.0, 48_000);
        let deep = freq(&run(DEEP, rate, &x)[9600..], rate);
        let high = freq(&run(CHIPMUNK, rate, &x)[9600..], rate);
        assert!((deep / 200.0 - DEEP_RATIO).abs() < 0.08, "deep {deep}");
        assert!((high / 200.0 - CHIPMUNK_RATIO).abs() < 0.12, "chipmunk {high}");
    }

    /// A vowel-like signal: `f0` Hz pulse train through two formant resonators.
    fn vowel(rate: f32, f0: f32, n: usize) -> Vec<f32> {
        let period = (rate / f0) as usize;
        let mut y: Vec<f64> = (0..n).map(|i| if i % period == 0 { 1.0 } else { 0.0 }).collect();
        for (f, bw) in [(700.0f64, 80.0f64), (1200.0, 90.0)] {
            let r = (-std::f64::consts::PI * bw / rate as f64).exp();
            let t = 2.0 * std::f64::consts::PI * f / rate as f64;
            let a = [1.0, -2.0 * r * t.cos(), r * r];
            let mut out = vec![0.0; n];
            lpc::all_pole(&a, &y, &mut out);
            y = out;
        }
        let peak = y.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        y.iter().map(|v| (v / peak * 0.5) as f32).collect()
    }

    /// Dominant period by autocorrelation (60..400 Hz), in samples.
    fn period(x: &[f32], rate: f32) -> f32 {
        let (lo, hi) = ((rate / 400.0) as usize, (rate / 60.0) as usize);
        let c = |l: usize| x.iter().zip(&x[l..]).map(|(p, q)| (p * q) as f64).sum::<f64>();
        (lo..hi).max_by(|&a, &b| c(a).partial_cmp(&c(b)).unwrap()).unwrap() as f32
    }

    #[test]
    fn anonymous_changes_pitch_and_keeps_level() {
        let rate = 48_000.0;
        let x = vowel(rate, 120.0, 96_000);
        for seed in [1234, 99] {
            let mut p = Processor::new(rate, seed);
            p.set_effect(ANONYMOUS);
            let mut y = x.clone();
            y.chunks_mut(128).for_each(|b| p.process(b));
            let (_, ratio) = anonymous_params(seed);
            let measured = period(&x[48_000..72_000], rate) / period(&y[48_000..72_000], rate);
            assert!((measured - ratio).abs() < 0.08, "seed {seed}: expected ratio {ratio}, got {measured}");
            let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
            let (a, b) = (rms(&x[48_000..]), rms(&y[48_000..]));
            assert!(b > a * 0.4 && b < a * 2.5, "level {a} -> {b}");
        }
    }

    #[test]
    fn switching_effects_never_breaks() {
        let rate = 48_000.0;
        let x = tone(rate, 180.0, 48_000);
        let mut p = Processor::new(rate, 7);
        let mut out = x.clone();
        for (i, block) in out.chunks_mut(128).enumerate() {
            p.set_effect((i / 20) as u32 % (EFFECT_COUNT + 1));
            p.process(block);
        }
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    }

    /// Deterministic white noise.
    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut r = XorShift(99);
        (0..n).map(|_| (r.next() * 2.0 - 1.0) * amp).collect()
    }

    #[test]
    fn denoise_removes_steady_noise_and_keeps_a_voice() {
        for rate in [48_000.0f32, 44_100.0] {
            // RNNoise adapts to white noise within a couple of seconds: measured after that.
            let n = rate as usize * 6;
            let hiss = noise(n, 0.05);
            let mut p = Processor::new(rate, 1);
            p.set_denoise(true);
            let mut y = hiss.clone();
            y.chunks_mut(128).for_each(|b| p.process(b));
            let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt();
            let (before, after) = (rms(&hiss[n * 2 / 3..]), rms(&y[n * 2 / 3..]));
            assert!(after < before * 0.3, "{rate}: noise {before} -> {after}");
            // A voiced sound with the same hiss keeps most of its level.
            let v = vowel(rate, 140.0, n);
            let mixed: Vec<f32> = v.iter().zip(&hiss).map(|(a, b)| a + b).collect();
            let mut p = Processor::new(rate, 1);
            p.set_denoise(true);
            let mut y = mixed.clone();
            y.chunks_mut(128).for_each(|b| p.process(b));
            assert!(y.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
            assert!(rms(&y[n / 2..]) > rms(&v[n / 2..]) * 0.3, "{rate}: voice lost");
        }
    }

    #[test]
    fn denoise_off_is_bit_exact_and_combines_with_effects() {
        let x = tone(48_000.0, 220.0, 4800);
        let mut p = Processor::new(48_000.0, 1);
        p.set_denoise(true);
        p.set_denoise(false);
        let mut y = x.clone();
        p.process(&mut y);
        assert_eq!(y, x);
        p.set_denoise(true);
        p.set_effect(ROBOT);
        let mut y = x.clone();
        y.chunks_mut(128).for_each(|b| p.process(b));
        assert!(y.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn anonymous_parameters_depend_on_the_seed() {
        let a = anonymous_params(1);
        let b = anonymous_params(2);
        assert!(a.0 != b.0 || a.1 != b.1);
        for s in 0..200 {
            let (alpha, pitch) = anonymous_params(s);
            assert!((0.75..=0.88).contains(&alpha));
            assert!((0.82..=0.9).contains(&pitch) || (1.12..=1.22).contains(&pitch));
        }
    }
}
