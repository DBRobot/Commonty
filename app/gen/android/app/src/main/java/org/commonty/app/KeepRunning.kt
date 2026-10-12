package org.commonty.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import java.io.File

// Keeps the app's process alive with no screen open: the pages it serves
// the browser (app/src/local.rs) and its place on the network are the Rust
// side's, which lives as long as the process. Android ends a process with
// nothing in front unless a service in it shows a notification.
class KeepRunning : Service() {
  companion object {
    private const val CHANNEL = "running"
    private const val ID = 1

    // the app's screen has started the Rust side in this process; started
    // at boot, the service is here before it, and says so
    @Volatile var appUp = false

    // the member's setting, the file the Rust side writes (app/src/background.rs)
    fun wanted(ctx: Context): Boolean {
      val f = File(ctx.dataDir, "start-at-login")
      return !f.exists() || f.readText().trim() != "off"
    }

    fun start(ctx: Context) {
      try {
        ContextCompat.startForegroundService(ctx, Intent(ctx, KeepRunning::class.java))
      } catch (e: Exception) {
        // Android refused a start from the background; the next one in front starts it
      }
    }

    fun stop(ctx: Context) {
      ctx.stopService(Intent(ctx, KeepRunning::class.java))
    }
  }

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (!wanted(this)) {
      stopSelf()
      return START_NOT_STICKY
    }
    val type = if (Build.VERSION.SDK_INT >= 34) ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
    try {
      ServiceCompat.startForeground(this, ID, notice(), type)
    } catch (e: Exception) {
      stopSelf()
      return START_NOT_STICKY
    }
    return START_STICKY
  }

  // quiet: no sound, no badge, low in the shade; a tap opens the app
  private fun notice(): android.app.Notification {
    if (Build.VERSION.SDK_INT >= 26) {
      val ch = NotificationChannel(CHANNEL, "Running in the background", NotificationManager.IMPORTANCE_MIN)
      ch.setShowBadge(false)
      getSystemService(NotificationManager::class.java).createNotificationChannel(ch)
    }
    val open = PendingIntent.getActivity(
      this, 0,
      Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )
    val (title, text) = if (appUp) {
      "Commonty is running" to "Your pages open in your browser on this phone."
    } else {
      "Commonty is ready" to "Tap to connect this phone to your boxes."
    }
    return NotificationCompat.Builder(this, CHANNEL)
      .setSmallIcon(R.drawable.ic_notice)
      .setContentTitle(title)
      .setContentText(text)
      .setContentIntent(open)
      .setOngoing(true)
      .setSilent(true)
      .setShowWhen(false)
      .setPriority(NotificationCompat.PRIORITY_MIN)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .build()
  }
}
