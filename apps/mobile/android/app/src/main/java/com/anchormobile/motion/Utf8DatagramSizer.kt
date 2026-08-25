package com.anchormobile.motion

object Utf8DatagramSizer {
  const val MAX_DATAGRAM_BYTES = 1024

  fun byteLength(value: String): Int = value.toByteArray(Charsets.UTF_8).size

  fun fitsDatagram(value: String): Boolean = byteLength(value) <= MAX_DATAGRAM_BYTES
}
