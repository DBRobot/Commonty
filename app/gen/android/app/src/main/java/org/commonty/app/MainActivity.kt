package org.commonty.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.AlertDialog
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat

// Not edge to edge: the pages draw from the top of the window, and the
// phone's own bar stays above them instead of over the header.
class MainActivity : TauriActivity() {
  // asked once, whatever the answer; the battery question follows it
  private val notices = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
    askBattery()
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    KeepRunning.appUp = true
    if (!KeepRunning.wanted(this)) return
    // started here, in front, where Android always allows it; the
    // notification then says the app is running
    KeepRunning.start(this)
    val asked = getSharedPreferences("background", MODE_PRIVATE)
    if (Build.VERSION.SDK_INT >= 33 &&
      ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
      !asked.getBoolean("notices", false)
    ) {
      asked.edit().putBoolean("notices", true).apply()
      notices.launch(Manifest.permission.POST_NOTIFICATIONS)
    } else {
      askBattery()
    }
  }

  // Android's battery saving stops apps in the background after a while;
  // asked once, in plain words, and the app carries on either way
  @SuppressLint("BatteryLife")
  private fun askBattery() {
    val asked = getSharedPreferences("background", MODE_PRIVATE)
    val power = getSystemService(PowerManager::class.java)
    if (asked.getBoolean("battery", false) || power.isIgnoringBatteryOptimizations(packageName)) return
    asked.edit().putBoolean("battery", true).apply()
    AlertDialog.Builder(this)
      .setTitle("Keep Commonty running?")
      .setMessage("Your pages open in your browser through Commonty. To save battery, Android may stop it when you are not using it. Let it keep running?")
      .setPositiveButton("Allow") { _, _ ->
        try {
          startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
          )
        } catch (e: Exception) {
          // no such screen on this phone: it keeps Android's default
        }
      }
      .setNegativeButton("Not now", null)
      .show()
  }
}
