package com.okenamobile

import android.content.Intent
import android.util.Log
import androidx.core.content.ContextCompat
import com.facebook.react.bridge.LifecycleEventListener
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.UiThreadUtil
import com.facebook.react.common.LifecycleState
import com.facebook.react.module.annotations.ReactModule

@ReactModule(name = ConnectionServiceModule.NAME)
class ConnectionServiceModule(context: ReactApplicationContext) :
  NativeConnectionServiceSpec(context), LifecycleEventListener {
  companion object {
    const val NAME = "OkenaConnectionService"
  }

  private var requested = false

  override fun getName() = NAME

  override fun initialize() {
    super.initialize()
    reactApplicationContext.addLifecycleEventListener(this)
    UiThreadUtil.runOnUiThread { ConnectionService.onDisconnect = { emitOnDisconnect(true) } }
  }

  override fun setActive(active: Boolean) {
    UiThreadUtil.runOnUiThread {
      if (requested == active && (active || !ConnectionService.running)) return@runOnUiThread
      requested = active
      updateService()
    }
  }

  override fun isRunning(): Boolean = ConnectionService.running

  private fun updateService() {
    val intent = Intent(reactApplicationContext, ConnectionService::class.java)
    if (!requested) {
      reactApplicationContext.stopService(intent)
    } else if (!ConnectionService.running && reactApplicationContext.lifecycleState == LifecycleState.RESUMED) {
      try {
        ContextCompat.startForegroundService(reactApplicationContext, intent)
      } catch (error: IllegalStateException) {
        Log.w("okena", "Could not start terminal connection service", error)
      } catch (error: SecurityException) {
        Log.w("okena", "Terminal connection service permission denied", error)
      }
    }
  }

  override fun onHostResume() = updateService()
  override fun onHostPause() = Unit
  override fun onHostDestroy() = Unit

  override fun invalidate() {
    reactApplicationContext.removeLifecycleEventListener(this)
    UiThreadUtil.runOnUiThread {
      ConnectionService.onDisconnect = null
      requested = false
      updateService()
    }
    super.invalidate()
  }
}
