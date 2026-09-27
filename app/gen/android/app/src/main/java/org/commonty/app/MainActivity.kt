package org.commonty.app

import android.os.Bundle

// Not edge to edge: the pages draw from the top of the window, and the
// phone's own bar stays above them instead of over the header.
class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
  }
}
