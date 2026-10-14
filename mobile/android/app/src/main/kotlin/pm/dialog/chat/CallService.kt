package pm.dialog.chat

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log

/**
 * Foreground service for the duration of a call (1:1 or group), Dart side:
 * mobile/lib/services/ongoing_call.dart (channel "dialog/call_service").
 *
 * Since Android 11 an app in the background (screen locked, another app on top) loses the
 * microphone and the camera unless a foreground service of type "microphone" / "camera" runs,
 * and a backgrounded process can be frozen or killed: the call would go silent and then drop.
 * The service shows an ongoing "Call in progress" notification with a "Hang up" action; tapping
 * it opens the app.
 *
 * Also holds the proximity wake lock of voice calls: the screen turns off at the ear (without
 * the app being paused) and back on when the phone is taken away.
 */
class CallService : Service() {
    companion object {
        private const val TAG = "CallService"
        private const val CHANNEL_ID = "ongoing_call"
        private const val NOTIFICATION_ID = 7301
        private const val ACTION_START = "pm.dialog.chat.call.START"
        private const val ACTION_HANGUP = "pm.dialog.chat.call.HANGUP"

        /** Set by MainActivity: the user tapped "Hang up" on the notification. */
        @Volatile
        var onHangUp: (() -> Unit)? = null

        private var proximity: PowerManager.WakeLock? = null

        fun start(context: Context, title: String, text: String, hangUp: String, channelName: String, video: Boolean) {
            val intent = Intent(context, CallService::class.java).apply {
                action = ACTION_START
                putExtra("title", title)
                putExtra("text", text)
                putExtra("hangUp", hangUp)
                putExtra("channelName", channelName)
                putExtra("video", video)
            }
            try {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                    context.startForegroundService(intent)
                } else {
                    context.startService(intent)
                }
            } catch (e: Exception) {
                // Not allowed from the background (Android 12+): the call goes on without it.
                Log.w(TAG, "cannot start the call service: $e")
            }
        }

        fun stop(context: Context) {
            setProximity(context, false)
            try {
                context.stopService(Intent(context, CallService::class.java))
            } catch (e: Exception) {
                Log.w(TAG, "cannot stop the call service: $e")
            }
        }

        /** Proximity wake lock (voice call on the earpiece): on / off. */
        @Synchronized
        fun setProximity(context: Context, on: Boolean) {
            if (on) {
                if (proximity?.isHeld == true) return
                val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
                if (!pm.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
                val lock = pm.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "dialog:call-proximity")
                lock.setReferenceCounted(false)
                try {
                    // Bounded: a call that somehow never ends does not keep it forever.
                    lock.acquire(4 * 60 * 60 * 1000L)
                    proximity = lock
                } catch (e: Exception) {
                    Log.w(TAG, "proximity wake lock: $e")
                }
            } else {
                val lock = proximity ?: return
                proximity = null
                try {
                    // Wait for the phone to leave the ear before the screen comes back on.
                    lock.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY)
                } catch (e: Exception) {
                    Log.w(TAG, "proximity wake lock release: $e")
                }
            }
        }
    }

    private var inForeground = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_HANGUP -> {
                val hangUp = onHangUp
                // A stale notification (no call any more): nothing to keep running.
                if (hangUp == null || !inForeground) stopSelf() else hangUp()
                return START_NOT_STICKY
            }
            ACTION_START -> {
                val notification = buildNotification(intent)
                inForeground = goForeground(notification, intent.getBooleanExtra("video", false))
                if (!inForeground) stopSelf()
            }
            // Restarted by the system without a call (should not happen with START_NOT_STICKY).
            else -> stopSelf()
        }
        return START_NOT_STICKY
    }

    /** The service types the permissions allow right now (microphone / camera need them granted). */
    private fun serviceTypes(video: Boolean): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return 0
        var types = ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            if (granted(Manifest.permission.RECORD_AUDIO)) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE
            if (video && granted(Manifest.permission.CAMERA)) types = types or ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA
        }
        return types
    }

    private fun granted(permission: String) = checkSelfPermission(permission) == PackageManager.PERMISSION_GRANTED

    private fun goForeground(notification: Notification, video: Boolean): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification)
            return true
        }
        // Full set first; then without the camera (e.g. started from the background, where
        // Android refuses it), the microphone alone, the phone-call type alone.
        val full = serviceTypes(video)
        val attempts = linkedSetOf(
            full,
            full and ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA.inv(),
            full and ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE,
            ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL,
        ).filter { it != 0 }
        for (types in attempts) {
            try {
                startForeground(NOTIFICATION_ID, notification, types)
                return true
            } catch (e: Exception) {
                Log.w(TAG, "startForeground(types=$types) refused: $e")
            }
        }
        return false
    }

    private fun buildNotification(intent: Intent): Notification {
        val title = intent.getStringExtra("title") ?: applicationInfo.loadLabel(packageManager).toString()
        val text = intent.getStringExtra("text") ?: ""
        val hangUp = intent.getStringExtra("hangUp") ?: "Hang up"
        val channelName = intent.getStringExtra("channelName") ?: "Calls"
        val immutable = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0

        val open = Intent(this, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val openPending = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT or immutable)
        val hangUpIntent = Intent(this, CallService::class.java).setAction(ACTION_HANGUP)
        val hangUpPending = PendingIntent.getService(this, 1, hangUpIntent, PendingIntent.FLAG_UPDATE_CURRENT or immutable)

        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            if (nm.getNotificationChannel(CHANNEL_ID) == null) {
                val channel = NotificationChannel(CHANNEL_ID, channelName, NotificationManager.IMPORTANCE_LOW).apply {
                    setShowBadge(false)
                    setSound(null, null)
                    enableVibration(false)
                    // Nothing about who is called on the lock screen.
                    lockscreenVisibility = Notification.VISIBILITY_PRIVATE
                }
                nm.createNotificationChannel(channel)
            } else {
                // Follows the app language.
                nm.getNotificationChannel(CHANNEL_ID)?.let {
                    if (it.name != channelName) {
                        it.name = channelName
                        nm.createNotificationChannel(it)
                    }
                }
            }
            Notification.Builder(this, CHANNEL_ID)
        } else {
            @Suppress("DEPRECATION")
            Notification.Builder(this).setPriority(Notification.PRIORITY_LOW)
        }
        builder
            .setSmallIcon(applicationInfo.icon)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openPending)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setShowWhen(true)
            .setUsesChronometer(true)
            .setCategory(Notification.CATEGORY_CALL)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }
        @Suppress("DEPRECATION")
        builder.addAction(Notification.Action.Builder(0, hangUp, hangUpPending).build())
        return builder.build()
    }

    override fun onDestroy() {
        inForeground = false
        setProximity(this, false)
        super.onDestroy()
    }
}
