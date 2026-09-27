package org.commonty.app

import android.app.Activity
import android.content.Intent
import android.net.VpnService
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
  var ip: String = ""
}

// Asks once for Android's VPN permission, starts the VPN service and hands
// its descriptor back to the Rust side (app/src/vpn.rs).
@TauriPlugin
class VpnPlugin(private val activity: Activity) : Plugin(activity) {
  private var ip = ""

  @Command
  fun start(invoke: Invoke) {
    ip = invoke.parseArgs(StartArgs::class.java).ip
    val consent = VpnService.prepare(activity)
    if (consent != null) {
      startActivityForResult(invoke, consent, "consented")
    } else {
      up(invoke)
    }
  }

  @ActivityCallback
  private fun consented(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode == Activity.RESULT_OK) up(invoke)
    else invoke.reject("the network needs the VPN permission")
  }

  private fun up(invoke: Invoke) {
    activity.startService(Intent(activity, CommontyVpn::class.java).putExtra("ip", ip))
    Thread {
      val fd = CommontyVpn.awaitFd(15000)
      if (fd < 0) invoke.reject("the VPN did not come up")
      else invoke.resolve(JSObject().put("fd", fd))
    }.start()
  }

  @Command
  fun stop(invoke: Invoke) {
    activity.startService(Intent(activity, CommontyVpn::class.java).setAction("stop"))
    invoke.resolve()
  }
}
