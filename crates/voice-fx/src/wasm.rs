//! C ABI for the WebAssembly build used by the desktop app's audio worklet
//! (app/src/lib/voiceFx.worklet.js). The worklet writes a block of samples into the
//! processor's buffer (`fx_buffer`), calls `fx_process` and reads the block back.

use crate::Processor;

/// Largest block `fx_process` accepts (AudioWorklet blocks are 128 samples).
const BLOCK: usize = 1024;

pub struct Handle {
    fx: Processor,
    buf: [f32; BLOCK],
}

#[no_mangle]
pub extern "C" fn fx_new(rate: f32, seed: u32) -> *mut Handle {
    Box::into_raw(Box::new(Handle { fx: Processor::new(rate, seed), buf: [0.0; BLOCK] }))
}

/// # Safety
/// `h` must come from `fx_new` and not be freed.
#[no_mangle]
pub unsafe extern "C" fn fx_buffer(h: *mut Handle) -> *mut f32 {
    (*h).buf.as_mut_ptr()
}

/// # Safety
/// `h` must come from `fx_new` and not be freed.
#[no_mangle]
pub unsafe extern "C" fn fx_set_effect(h: *mut Handle, effect: u32) {
    (*h).fx.set_effect(effect);
}

/// # Safety
/// `h` must come from `fx_new` and not be freed.
#[no_mangle]
pub unsafe extern "C" fn fx_set_denoise(h: *mut Handle, on: u32) {
    (*h).fx.set_denoise(on != 0);
}

/// # Safety
/// `h` must come from `fx_new` and not be freed.
#[no_mangle]
pub unsafe extern "C" fn fx_process(h: *mut Handle, n: u32) {
    let h = &mut *h;
    let n = (n as usize).min(BLOCK);
    h.fx.process(&mut h.buf[..n]);
}

/// # Safety
/// `h` must come from `fx_new`; it is invalid afterwards.
#[no_mangle]
pub unsafe extern "C" fn fx_free(h: *mut Handle) {
    drop(Box::from_raw(h));
}
