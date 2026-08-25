package com.anchormobile.motion

import java.net.DatagramPacket
import java.net.DatagramSocket

class UdpSocketTransport(
  private val socketFactory: () -> DatagramSocket = { DatagramSocket() },
) {
  private val lock = Any()
  private var generation = 0L
  private var socket: DatagramSocket? = null
  private var destination: UdpDestination? = null

  fun open(destination: UdpDestination) {
    val openState = synchronized(lock) {
      generation += 1
      val previousSocket = socket
      socket = null
      this.destination = null
      OpenState(previousSocket, generation)
    }
    openState.staleSocket?.close()

    val newSocket = socketFactory()
    val obsoleteSocket = synchronized(lock) {
      if (generation != openState.generation) {
        newSocket
      } else {
        socket = newSocket
        this.destination = destination
        null
      }
    }

    obsoleteSocket?.close()
  }

  fun send(payload: String) {
    val bytes = payload.toByteArray(Charsets.UTF_8)
    require(payload.isNotEmpty()) { "Payload must not be empty" }
    require(bytes.size <= Utf8DatagramSizer.MAX_DATAGRAM_BYTES) {
      "Payload exceeds ${Utf8DatagramSizer.MAX_DATAGRAM_BYTES} bytes"
    }

    val sendState = synchronized(lock) {
      val currentSocket = socket ?: throw IllegalStateException("Transport is not open")
      val currentDestination = destination ?: throw IllegalStateException("Transport is not open")
      SendState(generation, currentSocket, currentDestination)
    }

    val packet = DatagramPacket(
      bytes,
      bytes.size,
      sendState.destination.inetAddress,
      sendState.destination.port,
    )
    sendState.socket.send(packet)

    synchronized(lock) {
      if (generation != sendState.generation || socket !== sendState.socket || destination !== sendState.destination) {
        throw IllegalStateException("Transport is not open")
      }
    }
  }

  fun close() {
    val staleSocket = synchronized(lock) {
      generation += 1
      val previousSocket = socket
      socket = null
      destination = null
      previousSocket
    }
    staleSocket?.close()
  }

  internal fun installForTest(destination: UdpDestination, socket: DatagramSocket) {
    synchronized(lock) {
      generation += 1
      this.socket = socket
      this.destination = destination
    }
  }

  internal fun currentDestinationForTest(): UdpDestination? = synchronized(lock) { destination }

  private data class OpenState(
    val staleSocket: DatagramSocket?,
    val generation: Long,
  )

  private data class SendState(
    val generation: Long,
    val socket: DatagramSocket,
    val destination: UdpDestination,
  )
}
