package pm.dialog.chat

import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import io.flutter.embedding.android.FlutterFragmentActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

// FlutterFragmentActivity is required by local_auth (biometric app lock).
class MainActivity : FlutterFragmentActivity() {
    private val handler = Handler(Looper.getMainLooper())
    private var clearClipboard: Runnable? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        // Screen security is on by default (like Signal): set before the first frame so the
        // recent-apps thumbnail never shows content. Dart turns it off if the user disabled it.
        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
        super.onCreate(savedInstanceState)
        showOverLockScreenForCall(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        showOverLockScreenForCall(intent)
    }

    /** Opened from an incoming-call notification (full-screen intent or Answer): show over the
     *  lock screen and turn the screen on. Never otherwise, so the app does not appear over the
     *  lock screen by itself; Dart turns it off again when the call ends. */
    private fun showOverLockScreenForCall(intent: Intent?) {
        val payload = intent?.getStringExtra("payload") ?: return
        if (payload.startsWith("call:")) setShowWhenLockedCompat(true)
    }

    @Suppress("DEPRECATION")
    private fun setShowWhenLockedCompat(on: Boolean) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O_MR1) {
            setShowWhenLocked(on)
            setTurnScreenOn(on)
        } else {
            val flags = WindowManager.LayoutParams.FLAG_SHOW_WHEN_LOCKED or WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON
            if (on) window.addFlags(flags) else window.clearFlags(flags)
        }
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // Ongoing call: foreground service + proximity wake lock (CallService.kt,
        // mobile/lib/services/ongoing_call.dart).
        val callChannel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dialog/call_service")
        CallService.onHangUp = { handler.post { callChannel.invokeMethod("hangUp", null) } }
        callChannel.setMethodCallHandler { call, result ->
            when (call.method) {
                "start" -> {
                    CallService.start(
                        applicationContext,
                        title = call.argument<String>("title") ?: applicationInfo.loadLabel(packageManager).toString(),
                        text = call.argument<String>("text") ?: "",
                        hangUp = call.argument<String>("hangUp") ?: "Hang up",
                        channelName = call.argument<String>("channelName") ?: "Calls",
                        video = call.argument<Boolean>("video") ?: false,
                    )
                    result.success(null)
                }
                "stop" -> {
                    CallService.stop(applicationContext)
                    result.success(null)
                }
                "setProximity" -> {
                    CallService.setProximity(applicationContext, call.arguments == true)
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
        // Live voice effects in calls (mobile/lib/services/voice_fx.dart).
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dialog/voicefx").setMethodCallHandler { call, result ->
            try {
                when (call.method) {
                    "configure" -> {
                        VoiceFx.configure(call.argument<Int>("effect") ?: 0, call.argument<Boolean>("denoise") ?: false)
                        result.success(VoiceFx.register())
                    }
                    "newCall" -> {
                        VoiceFx.newCall(call.argument<Int>("seed") ?: 1)
                        result.success(VoiceFx.register())
                    }
                    else -> result.notImplemented()
                }
            } catch (e: Throwable) {
                result.error("voicefx", e.message, null)
            }
        }
        // Battery optimization exemption (mobile/lib/services/battery.dart).
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dialog/battery").setMethodCallHandler { call, result ->
            val power = getSystemService(android.content.Context.POWER_SERVICE) as android.os.PowerManager
            when (call.method) {
                "isIgnoring" -> result.success(power.isIgnoringBatteryOptimizations(packageName))
                "request" -> {
                    try {
                        startActivity(
                            Intent(android.provider.Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                                .setData(android.net.Uri.parse("package:$packageName"))
                        )
                    } catch (e: Exception) {
                        // Some OEM builds lack the dialog: open the app's settings instead.
                        startActivity(
                            Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
                                .setData(android.net.Uri.parse("package:$packageName"))
                        )
                    }
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dialog/security").setMethodCallHandler { call, result ->
            when (call.method) {
                "setScreenSecurity" -> {
                    if (call.arguments == true) {
                        window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
                    } else {
                        window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
                    }
                    result.success(null)
                }
                "setShowWhenLocked" -> {
                    setShowWhenLockedCompat(call.arguments == true)
                    result.success(null)
                }
                "copySensitive" -> {
                    val text = call.argument<String>("text") ?: ""
                    val seconds = call.argument<Int>("seconds") ?: 60
                    copySensitive(text, seconds)
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
    }

    override fun onDestroy() {
        // The engine (and any call) goes with the activity.
        if (isFinishing) {
            CallService.onHangUp = null
            CallService.stop(applicationContext)
        }
        super.onDestroy()
    }

    /** Copies [text] marked as sensitive and clears it after [seconds] if it is still on the
     *  clipboard. The clearing is scheduled with AlarmManager (survives backgrounding / process
     *  death, see [SensitiveClipboard]); the Runnable below only makes it punctual while we live. */
    private fun copySensitive(text: String, seconds: Int) {
        val (token, hash) = SensitiveClipboard.copy(this, text, seconds)
        clearClipboard?.let { handler.removeCallbacks(it) }
        val task = Runnable {
            clearClipboard = null
            SensitiveClipboard.clearIfOurs(this, token, hash)
        }
        clearClipboard = task
        handler.postDelayed(task, seconds * 1000L)
    }
}
