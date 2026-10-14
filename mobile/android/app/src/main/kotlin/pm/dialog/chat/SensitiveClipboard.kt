package pm.dialog.chat

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.PersistableBundle
import android.os.SystemClock
import java.security.MessageDigest
import java.util.UUID

/**
 * Sensitive clipboard copies that are cleared after a delay even if the app is backgrounded or
 * its process is killed.
 *
 * Scheduling: a one-shot inexact AlarmManager alarm (setAndAllowWhileIdle, ELAPSED_REALTIME_WAKEUP)
 * delivers [ClipboardClearReceiver] (manifest-registered, exported=false). While the activity is
 * alive, MainActivity also posts an in-process Runnable for punctual clearing; both call [clearIfOurs],
 * which is idempotent (the pending token is consumed by the first run).
 *
 * "Is the clip still ours?" — no plaintext is stored anywhere:
 *  - each copy gets a random token, put in the ClipDescription extras; the token and the copy time
 *    (elapsedRealtime) are kept in private SharedPreferences, the SHA-256 of the text travels only in
 *    the alarm's PendingIntent extras (held by the system, never written to disk by us);
 *  - foreground (or Android < 10): the clipboard is readable, so we compare the description token
 *    and/or the SHA-256 of the current text; a different clip is left alone;
 *  - background on Android 10+: getPrimaryClip() AND getPrimaryClipDescription() both return null
 *    (only the focused app / default IME may read either), so nothing can be verified. We then clear
 *    unconditionally only if this token is still the last sensitive copy we made and less than
 *    [MAX_BLIND_CLEAR_MS] passed since it was set (a late alarm out of Doze must not wipe something
 *    the user copied much later). Writing/clearing the clipboard is allowed from the background.
 *  - clearPrimaryClip() on API 28+, otherwise an empty-ish clip replaces it.
 */
object SensitiveClipboard {
    private const val PREFS = "dialog_sensitive_clipboard"
    private const val KEY_TOKEN = "token"
    private const val KEY_SET_AT = "setAt"
    const val EXTRA_TOKEN = "pm.dialog.chat.CLIP_TOKEN"
    const val EXTRA_HASH = "pm.dialog.chat.CLIP_HASH"
    private const val MAX_BLIND_CLEAR_MS = 5 * 60 * 1000L
    private const val REQUEST_CODE = 0x434C4950 // "CLIP"

    /** Copies [text] marked sensitive and schedules its clearing; returns (token, hash). */
    fun copy(context: Context, text: String, seconds: Int): Pair<String, String> {
        val app = context.applicationContext
        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val token = UUID.randomUUID().toString()
        val hash = sha256(text)
        val clip = ClipData.newPlainText(app.applicationInfo.loadLabel(app.packageManager), text)
        val extras = PersistableBundle()
        // Android 13+ hides sensitive clips from the clipboard preview and keyboard suggestions;
        // the literal key is honoured by some OEM keyboards on older versions.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            extras.putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true)
        } else {
            extras.putBoolean("android.content.extra.IS_SENSITIVE", true)
        }
        extras.putString(EXTRA_TOKEN, token)
        clip.description.extras = extras
        cm.setPrimaryClip(clip)

        app.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY_TOKEN, token)
            .putLong(KEY_SET_AT, SystemClock.elapsedRealtime())
            .apply()

        try {
            val am = app.getSystemService(Context.ALARM_SERVICE) as AlarmManager
            val at = SystemClock.elapsedRealtime() + seconds * 1000L
            // Same request code: a newer copy replaces the previous pending alarm.
            am.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pendingIntent(app, token, hash))
        } catch (_: Exception) {
            // The in-process Runnable in MainActivity still covers the foreground case.
        }
        return Pair(token, hash)
    }

    private fun pendingIntent(context: Context, token: String, hash: String): PendingIntent {
        val intent = Intent(context, ClipboardClearReceiver::class.java)
            .putExtra(EXTRA_TOKEN, token)
            .putExtra(EXTRA_HASH, hash)
        return PendingIntent.getBroadcast(
            context, REQUEST_CODE, intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    /** Clears the clipboard if it still holds the sensitive copy identified by [token]/[hash]. */
    fun clearIfOurs(context: Context, token: String, hash: String?) {
        val app = context.applicationContext
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        // Not the latest sensitive copy, or already handled: nothing to do.
        if (prefs.getString(KEY_TOKEN, null) != token) return
        val setAt = prefs.getLong(KEY_SET_AT, 0L)
        prefs.edit().remove(KEY_TOKEN).remove(KEY_SET_AT).apply()

        val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val description = try { cm.primaryClipDescription } catch (_: Exception) { null }
        val clip = try { cm.primaryClip } catch (_: Exception) { null }
        val readable = description != null || clip != null
        val ours = if (readable) {
            val clipToken = description?.extras?.getString(EXTRA_TOKEN)
            val text = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
            (clipToken != null && clipToken == token) ||
                (clipToken == null && hash != null && text != null && sha256(text) == hash)
        } else {
            val age = SystemClock.elapsedRealtime() - setAt
            age in 0..MAX_BLIND_CLEAR_MS
        }
        if (!ours) return
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
                cm.clearPrimaryClip()
            } else {
                cm.setPrimaryClip(ClipData.newPlainText("", " "))
            }
        } catch (_: Exception) {}
    }

    private fun sha256(text: String): String =
        MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
            .joinToString("") { "%02x".format(it) }
}

/** Alarm target for [SensitiveClipboard]; not exported. */
class ClipboardClearReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val token = intent.getStringExtra(SensitiveClipboard.EXTRA_TOKEN) ?: return
        SensitiveClipboard.clearIfOurs(context, token, intent.getStringExtra(SensitiveClipboard.EXTRA_HASH))
    }
}
