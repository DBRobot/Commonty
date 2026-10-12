package org.commonty.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

// The phone started, or the app was updated: back in the background, as
// the member's setting says
class StartAtBoot : BroadcastReceiver() {
  override fun onReceive(ctx: Context, intent: Intent) {
    when (intent.action) {
      Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED ->
        if (KeepRunning.wanted(ctx)) KeepRunning.start(ctx)
    }
  }
}
