package com.okena.terminalview

import com.facebook.react.ReactPackage
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.uimanager.ViewManager

/** Autolinked package that registers [OkenaTerminalViewManager]. */
class OkenaTerminalViewPackage : ReactPackage {
  override fun createViewManagers(
    reactContext: ReactApplicationContext,
  ): List<ViewManager<in Nothing, in Nothing>> = listOf(OkenaTerminalViewManager())
}
