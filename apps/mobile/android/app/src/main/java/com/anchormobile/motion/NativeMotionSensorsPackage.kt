package com.anchormobile.motion

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider

class NativeMotionSensorsPackage : BaseReactPackage() {
  override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? =
    if (name == NativeMotionSensorsModule.NAME) {
      NativeMotionSensorsModule(reactContext)
    } else if (name == NativeUdpSenderModule.NAME) {
      NativeUdpSenderModule(reactContext)
    } else {
      null
    }

  override fun getReactModuleInfoProvider(): ReactModuleInfoProvider = ReactModuleInfoProvider {
    mapOf(
      NativeMotionSensorsModule.NAME to ReactModuleInfo(
        name = NativeMotionSensorsModule.NAME,
        className = NativeMotionSensorsModule.NAME,
        canOverrideExistingModule = false,
        needsEagerInit = false,
        isCxxModule = false,
        isTurboModule = true,
      ),
      NativeUdpSenderModule.NAME to ReactModuleInfo(
        name = NativeUdpSenderModule.NAME,
        className = NativeUdpSenderModule.NAME,
        canOverrideExistingModule = false,
        needsEagerInit = false,
        isCxxModule = false,
        isTurboModule = true,
      ),
    )
  }
}
