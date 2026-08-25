package com.anchormobile.motion

internal object NativeUdpSenderPortNormalizer {
  fun normalize(port: Double): Int {
    require(port.isFinite() && port % 1.0 == 0.0) { "Invalid destination port" }
    val normalizedPort = port.toInt()
    require(normalizedPort in 1..65535) { "Invalid destination port" }
    return normalizedPort
  }
}
