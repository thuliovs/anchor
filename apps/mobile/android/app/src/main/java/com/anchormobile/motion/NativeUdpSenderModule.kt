package com.anchormobile.motion

import com.facebook.fbreact.specs.NativeUdpSenderSpec
import com.facebook.react.bridge.LifecycleEventListener
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.module.annotations.ReactModule

@ReactModule(name = NativeUdpSenderModule.NAME)
class NativeUdpSenderModule(
  reactContext: ReactApplicationContext,
) : NativeUdpSenderSpec(reactContext), LifecycleEventListener {
  private val controller = NativeUdpSenderController()

  init {
    reactApplicationContext.addLifecycleEventListener(this)
  }

  override fun getName(): String = NAME

  @ReactMethod
  override fun open(host: String, port: Double, promise: Promise) {
    val normalizedPort = try {
      NativeUdpSenderPortNormalizer.normalize(port)
    } catch (error: IllegalArgumentException) {
      promise.reject(NativeUdpSenderController.ERR_INVALID_DESTINATION, error.message)
      return
    }

    controller.open(host, normalizedPort, PromiseAdapter(promise))
  }

  @ReactMethod
  override fun send(payload: String, promise: Promise) {
    controller.send(payload, PromiseAdapter(promise))
  }

  @ReactMethod
  override fun close() {
    controller.close()
  }

  override fun onHostResume() {
    controller.onHostResume()
  }

  override fun onHostPause() {
    controller.onHostPause()
  }

  override fun onHostDestroy() {
    controller.onHostDestroy()
  }

  override fun invalidate() {
    controller.invalidate()
    reactApplicationContext.removeLifecycleEventListener(this)
    super.invalidate()
  }
  companion object {
    const val NAME = "NativeUdpSender"
  }
}

private class PromiseAdapter(
  private val promise: Promise,
) : PromiseSink {
  override fun resolve(value: Any?) {
    promise.resolve(value)
  }

  override fun reject(code: String, message: String?) {
    promise.reject(code, message)
  }
}
