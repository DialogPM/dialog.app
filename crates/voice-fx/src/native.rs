//! Live effects on a native audio thread (the mobile app's call microphone, through
//! flutter_webrtc's capture post-processing: Android `AudioProcessingAdapter`, iOS
//! `AudioManager.capturePostProcessingAdapter`). There is one microphone pipeline, so the state
//! is global: the UI thread sets the wanted effect (`configure`, `new_call`), the audio thread
//! applies it on the next block (`process_scaled`). With no effect and no noise reduction the
//! samples are not touched and no lock is taken.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use crate::{Processor, NONE};

static EFFECT: AtomicU32 = AtomicU32::new(NONE);
static DENOISE: AtomicBool = AtomicBool::new(false);
static SEED: AtomicU32 = AtomicU32::new(1);
/// Bumped by `new_call`: the processor starts afresh (new random anonymous voice).
static EPOCH: AtomicU32 = AtomicU32::new(0);

struct State {
    rate: u32,
    epoch: u32,
    fx: Processor,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The effect (crate constants) and noise reduction to apply from the next block on.
pub fn configure(effect: u32, denoise: bool) {
    EFFECT.store(effect, Ordering::Relaxed);
    DENOISE.store(denoise, Ordering::Relaxed);
}

/// A new call: a fresh processor with a new random seed (anonymous voice parameters).
pub fn new_call(seed: u32) {
    SEED.store(seed.max(1), Ordering::Relaxed);
    EPOCH.fetch_add(1, Ordering::Relaxed);
}

/// Processes `buf` in place. Samples are divided by `scale` before processing and multiplied
/// back after (WebRTC's audio buffers hold floats in the 16-bit range: scale 32768).
pub fn process_scaled(buf: &mut [f32], rate: u32, scale: f32) {
    let effect = EFFECT.load(Ordering::Relaxed);
    let denoise = DENOISE.load(Ordering::Relaxed);
    if effect == NONE && !denoise {
        return;
    }
    if rate == 0 || buf.is_empty() || !(scale > 0.0) {
        return;
    }
    // Never block the audio thread: a busy lock (a concurrent reset) skips this block.
    let Ok(mut guard) = STATE.try_lock() else { return };
    let epoch = EPOCH.load(Ordering::Relaxed);
    let fresh = !matches!(&*guard, Some(s) if s.rate == rate && s.epoch == epoch);
    if fresh {
        *guard = Some(State { rate, epoch, fx: Processor::new(rate as f32, SEED.load(Ordering::Relaxed)) });
    }
    let Some(state) = guard.as_mut() else { return };
    state.fx.set_effect(effect);
    state.fx.set_denoise(denoise);
    let inv = 1.0 / scale;
    buf.iter_mut().for_each(|v| *v *= inv);
    state.fx.process(buf);
    buf.iter_mut().for_each(|v| *v *= scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_until_configured_then_processes() {
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.05).sin() * 8000.0).collect();
        let mut y = x.clone();
        configure(NONE, false);
        y.chunks_mut(480).for_each(|b| process_scaled(b, 48_000, 32768.0));
        assert_eq!(y, x);
        new_call(42);
        configure(crate::ROBOT, false);
        y.chunks_mut(480).for_each(|b| process_scaled(b, 48_000, 32768.0));
        assert_ne!(y, x);
        assert!(y.iter().all(|v| v.is_finite() && v.abs() <= 32768.0));
        configure(NONE, false);
    }
}
