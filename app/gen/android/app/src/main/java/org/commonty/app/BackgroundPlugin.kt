package org.commonty.app

import android.app.Activity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin

@InvokeArg
class SetArgs {
  var on: Boolean = true
}

// The setting's switch in the app (app/src/background.rs), which has
// already written it down: the service started or stopped to match.
@TauriPlugin
class BackgroundPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun set(invoke: Invoke) {
    if (invoke.parseArgs(SetArgs::class.java).on) KeepRunning.start(activity)
    else KeepRunning.stop(activity)
    invoke.resolve()
  }
}
