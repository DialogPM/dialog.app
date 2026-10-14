//! Native entry points of the live voice effects (crates/voice-fx/src/native.rs) for the call
//! microphone: called by the app's audio processing hooks on the WebRTC capture thread
//! (iOS: AppDelegate.swift, C ABI; Android: VoiceFx.kt, JNI) and by the effect controls
//! (Flutter, through a method channel to the same native code).

use voice_fx::native;

/// iOS: effect (voice-fx constants) and noise reduction from the next audio block on.
#[no_mangle]
pub extern "C" fn dialog_fx_configure(effect: u32, denoise: u32) {
    native::configure(effect, denoise != 0);
}

/// iOS: a new call (new random anonymous voice).
#[no_mangle]
pub extern "C" fn dialog_fx_new_call(seed: u32) {
    native::new_call(seed);
}

/// iOS: processes one block of the microphone in place (floats in the 16-bit range).
///
/// # Safety
/// `buf` must point to `n` writable floats.
#[no_mangle]
pub unsafe extern "C" fn dialog_fx_process(buf: *mut f32, n: u32, rate: u32) {
    if buf.is_null() || n == 0 {
        return;
    }
    native::process_scaled(std::slice::from_raw_parts_mut(buf, n as usize), rate, 32768.0);
}

/// Android (VoiceFx.kt). Compiled on every platform; only Android calls these.
mod android {
    use jni_sys::{jboolean, jclass, jint, jobject, JNIEnv};
    use voice_fx::native;

    #[no_mangle]
    pub extern "system" fn Java_pm_dialog_chat_VoiceFx_nativeConfigure(_env: *mut JNIEnv, _cls: jclass, effect: jint, denoise: jboolean) {
        native::configure(effect.max(0) as u32, denoise != 0);
    }

    #[no_mangle]
    pub extern "system" fn Java_pm_dialog_chat_VoiceFx_nativeNewCall(_env: *mut JNIEnv, _cls: jclass, seed: jint) {
        native::new_call(seed as u32);
    }

    /// `buffer`: direct ByteBuffer of `frames` floats (16-bit range) from flutter_webrtc's
    /// capture post-processing, processed in place.
    ///
    /// # Safety
    /// Called by the JVM with a valid environment and a direct buffer.
    #[no_mangle]
    pub unsafe extern "system" fn Java_pm_dialog_chat_VoiceFx_nativeProcess(env: *mut JNIEnv, _cls: jclass, buffer: jobject, frames: jint, rate: jint) {
        if env.is_null() || buffer.is_null() || frames <= 0 || rate <= 0 {
            return;
        }
        let (Some(address), Some(capacity)) = ((**env).GetDirectBufferAddress, (**env).GetDirectBufferCapacity) else { return };
        let ptr = address(env, buffer) as *mut f32;
        let bytes = capacity(env, buffer);
        if ptr.is_null() || bytes < 4 {
            return;
        }
        let n = (frames as usize).min(bytes as usize / 4);
        native::process_scaled(std::slice::from_raw_parts_mut(ptr, n), rate as u32, 32768.0);
    }
}
