package com.anchormobile.motion

import java.net.InetAddress

data class UdpDestination(
  val hostAddress: String,
  val port: Int,
  val inetAddress: InetAddress,
) {
  companion object {
    fun parse(host: String, port: Int): UdpDestination {
      val normalizedHost = host.trim()
      val octets = normalizedHost.split('.')
      require(octets.size == 4) { "Invalid destination host" }

      val bytes = ByteArray(4)
      octets.forEachIndexed { index, part ->
        require(part.isNotEmpty() && part.all { it.isDigit() }) { "Invalid destination host" }
        val value = part.toIntOrNull() ?: throw IllegalArgumentException("Invalid destination host")
        require(value in 0..255) { "Invalid destination host" }
        bytes[index] = value.toByte()
      }

      require(port in 1..65535) { "Invalid destination port" }
      require(bytes[0].toInt() and 0xFF != 0) { "Invalid destination host" }
      require(normalizedHost != "255.255.255.255") { "Invalid destination host" }
      require(bytes[0].toInt() and 0xFF !in 224..239) { "Invalid destination host" }
      require(bytes[0].toInt() and 0xFF !in 240..255) { "Invalid destination host" }

      return UdpDestination(
        hostAddress = normalizedHost,
        port = port,
        inetAddress = InetAddress.getByAddress(normalizedHost, bytes),
      )
    }
  }
}
