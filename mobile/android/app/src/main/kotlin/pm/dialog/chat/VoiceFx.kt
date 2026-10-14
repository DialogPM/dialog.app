package pm.dialog.chat

import com.cloudwebrtc.webrtc.FlutterWebRTCPlugin
import com.cloudwebrtc.webrtc.audio.AudioProcessingAdapter
import java.nio.ByteBuffer

/**
 * Live voice effects and noise reduction of the call microphone (crates/voice-fx, through
 * mobile/rust/src/voice_fx.rs). The processor sits in flutter_webrtc's capture post-processing,
 * after echo cancellation, so the other side only ever receives the processed voice. With no
 * effect and no noise reduction the samples pass through untouched.
 */
object VoiceFx : AudioProcessingAdapter.ExternalAudioFrameProcessing {
    init {
        // The Rust library (also loaded by Flutter) holds the JNI functions below.
        System.loadLibrary("chat_bridge")
    }

    @Volatile private var rate = 48000
    @Volatile private var registered = false

    @JvmStatic private external fun nativeConfigure(effect: Int, denoise: Boolean)
    @JvmStatic private external fun nativeNewCall(seed: Int)
    @JvmStatic private external fun nativeProcess(buffer: ByteBuffer, frames: Int, rate: Int)

    /** Hooks into the WebRTC microphone pipeline (once, after the plugin is attached). */
    @Synchronized
    fun register(): Boolean {
        if (registered) return true
        val controller = FlutterWebRTCPlugin.sharedSingleton?.audioProcessingController ?: return false
        controller.capturePostProcessing.addProcessor(this)
        registered = true
        return true
    }

    fun configure(effect: Int, denoise: Boolean) {
        register()
        nativeConfigure(effect, denoise)
    }

    fun newCall(seed: Int) {
        register()
        nativeNewCall(seed)
    }

    override fun initialize(sampleRateHz: Int, numChannels: Int) {
        rate = sampleRateHz
    }

    override fun reset(newRate: Int) {
        rate = newRate
    }

    override fun process(numBands: Int, numFrames: Int, buffer: ByteBuffer) {
        if (buffer.isDirect) nativeProcess(buffer, numFrames, rate)
    }
}
