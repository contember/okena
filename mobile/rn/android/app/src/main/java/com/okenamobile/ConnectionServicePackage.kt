package com.okenamobile

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class ConnectionServicePackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == ConnectionServiceModule.NAME) ConnectionServiceModule(reactContext) else null

  override fun getReactModuleInfoProvider() = ReactModuleInfoProvider {
    mapOf(
      ConnectionServiceModule.NAME to ReactModuleInfo(
        ConnectionServiceModule.NAME,
        ConnectionServiceModule.NAME,
        false,
        false,
        false,
        true,
      ),
    )
  }
}
