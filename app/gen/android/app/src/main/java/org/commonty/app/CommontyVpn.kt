package org.commonty.app

import android.content.Intent
import android.net.VpnService

// The phone's way onto the fleet's network for everything on it, the
// browser included: an interface that claims only the network's addresses,
// with the engine's resolver (100.100.100.100) answering names, the fleet's
// from the network and the rest from the resolvers the network names. The
// descriptor goes to the engine (app/net); this only builds it.
class CommontyVpn : VpnService() {
  companion object {
    private val lock = Object()
    private var fd = -1

    // the interface's descriptor once the service has built it, or -1
    fun awaitFd(ms: Long): Int {
      synchronized(lock) {
        val end = System.currentTimeMillis() + ms
        while (fd < 0) {
          val left = end - System.currentTimeMillis()
          if (left <= 0) break
          lock.wait(left)
        }
        val f = fd
        fd = -1
        return f
      }
    }
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    if (intent?.action == "stop") {
      stopSelf()
      return START_NOT_STICKY
    }
    val ip = intent?.getStringExtra("ip") ?: return START_NOT_STICKY
    val pfd = Builder()
      .setSession("Commonty")
      .setMtu(1280)
      .addAddress(ip, 32)
      .addRoute("100.64.0.0", 10)
      .addDnsServer("100.100.100.100")
      .establish()
    synchronized(lock) {
      // the engine owns it from here, and closes it when it stops
      fd = pfd?.detachFd() ?: -1
      lock.notifyAll()
    }
    return START_STICKY
  }
}
