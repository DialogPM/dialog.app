//! LPC analysis and the McAdams pole-angle warp, without allocations (fixed-size buffers), so
//! it can run on a real-time audio thread. Used by the live "anonymous" effect
//! ([`crate::Mcadams`]) and by the voice note masking of chat-core (voice_mask.rs).

use std::f64::consts::PI;

/// Highest supported LPC order.
pub const MAX_ORDER: usize = 32;

/// Periodic Hann window value (the windows sum to 1 at 50% overlap).
pub fn hann(i: usize, n: usize) -> f64 {
    0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos()
}

/// LPC coefficients `a = [1, a1..ap]` of `frame` (autocorrelation + Levinson-Durbin), with
/// `a.len() - 1` the order. False for a silent or ill-conditioned frame.
pub fn lpc(frame: &[f64], a: &mut [f64]) -> bool {
    let order = a.len() - 1;
    debug_assert!(order <= MAX_ORDER);
    let mut r = [0.0f64; MAX_ORDER + 1];
    for (k, rk) in r.iter_mut().enumerate().take(order + 1) {
        *rk = frame.iter().zip(&frame[k.min(frame.len())..]).map(|(x, y)| x * y).sum();
    }
    if r[0] < 1e-9 {
        return false;
    }
    a.iter_mut().for_each(|v| *v = 0.0);
    a[0] = 1.0;
    let mut prev = [0.0f64; MAX_ORDER + 1];
    let mut err = r[0] * (1.0 + 1e-9);
    for i in 1..=order {
        let mut acc = r[i];
        for j in 1..i {
            acc += a[j] * r[i - j];
        }
        let k = -acc / err;
        prev[..i].copy_from_slice(&a[..i]);
        for j in 1..i {
            a[j] = prev[j] + k * prev[i - j];
        }
        a[i] = k;
        err *= 1.0 - k * k;
        if err <= 0.0 {
            return false;
        }
    }
    true
}

/// y = A(z) x, zero initial state.
pub fn fir(a: &[f64], x: &[f64], y: &mut [f64]) {
    for n in 0..x.len() {
        y[n] = a.iter().enumerate().take(n + 1).map(|(k, c)| c * x[n - k]).sum();
    }
}

/// y = x / A(z), zero initial state (a[0] = 1).
pub fn all_pole(a: &[f64], x: &[f64], y: &mut [f64]) {
    for n in 0..x.len() {
        let mut v = x[n];
        for k in 1..a.len().min(n + 1) {
            v -= a[k] * y[n - k];
        }
        y[n] = v;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct C(pub f64, pub f64);

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

/// Roots of the monic polynomial `a` (Durand-Kerner) into `z[..p]`. When `warm` is set, `z`
/// already holds a starting guess (the previous frame's roots: formants move slowly, so this
/// converges in a few iterations).
fn roots(a: &[f64], z: &mut [C], warm: bool) -> bool {
    let p = a.len() - 1;
    let eval = |x: C| a.iter().fold(C(0.0, 0.0), |acc, &c| acc.mul(x).add(C(c, 0.0)));
    if !warm {
        for (k, zk) in z.iter_mut().enumerate().take(p) {
            *zk = C::polar(0.9, 2.0 * PI * (k as f64 + 0.25) / p as f64);
        }
    }
    for _ in 0..300 {
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
        if moved < 1e-10 {
            break;
        }
    }
    z[..p].iter().all(|r| r.0.is_finite() && r.1.is_finite())
}

/// Root-finding state kept between frames (warm start).
pub struct PoleWarp {
    z: [C; MAX_ORDER],
    warm: bool,
}

impl Default for PoleWarp {
    fn default() -> Self {
        PoleWarp { z: [C::default(); MAX_ORDER], warm: false }
    }
}

impl PoleWarp {
    /// Writes into `out` (same length as `a`) the polynomial whose complex poles have their
    /// angle φ moved to φ^alpha (sign kept, so conjugate pairs stay conjugate). Radii are kept
    /// (capped below 1), so the all-pole filter stays stable. Returns false (and copies `a`)
    /// when the roots could not be found.
    pub fn warp(&mut self, a: &[f64], alpha: f64, out: &mut [f64]) -> bool {
        let p = a.len() - 1;
        if !roots(a, &mut self.z, self.warm) {
            self.warm = false;
            out.copy_from_slice(a);
            return false;
        }
        self.warm = true;
        let mut poly = [C::default(); MAX_ORDER + 1];
        poly[0] = C(1.0, 0.0);
        let mut len = 1;
        for r in &self.z[..p] {
            let t = r.arg();
            let w = if r.1.abs() < 1e-9 { C(r.0.clamp(-0.999, 0.999), 0.0) } else { C::polar(r.abs().min(0.999), t.signum() * t.abs().powf(alpha)) };
            // poly *= (z - w)
            poly[len] = C(0.0, 0.0);
            for i in (1..=len).rev() {
                poly[i] = poly[i].sub(poly[i - 1].mul(w));
            }
            len += 1;
        }
        for (o, c) in out.iter_mut().zip(&poly[..=p]) {
            *o = c.0;
        }
        true
    }
}
