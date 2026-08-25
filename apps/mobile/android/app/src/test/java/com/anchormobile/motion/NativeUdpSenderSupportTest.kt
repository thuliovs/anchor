package com.anchormobile.motion

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeUdpSenderSupportTest {
  @Test
  fun `parser aceita ipv4 unicast valido`() {
    val destination = UdpDestination.parse(host = "192.168.0.10", port = 57421)

    assertEquals("192.168.0.10", destination.hostAddress)
    assertEquals(57421, destination.port)
  }

  @Test
  fun `parser rejeita ipv4 invalido multicast e zero`() {
    val invalidHosts = listOf("", "300.1.1.1", "0.0.0.0", "0.0.0.1", "224.0.0.1", "239.1.2.3", "240.0.0.1", "::1")

    invalidHosts.forEach { host ->
      try {
        UdpDestination.parse(host = host, port = 57421)
        throw AssertionError("expected destination to fail for $host")
      } catch (error: IllegalArgumentException) {
        assertTrue(error.message?.contains("destination", ignoreCase = true) == true)
      }
    }
  }

  @Test
  fun `portas limite sao aceitas e fora da faixa sao rejeitadas`() {
    assertEquals(1, UdpDestination.parse(host = "10.0.0.2", port = 1).port)
    assertEquals(65535, UdpDestination.parse(host = "10.0.0.2", port = 65535).port)

    for (invalidPort in listOf(0, -1, 65536)) {
      try {
        UdpDestination.parse(host = "10.0.0.2", port = invalidPort)
        throw AssertionError("expected invalid port $invalidPort")
      } catch (error: IllegalArgumentException) {
        assertTrue(error.message?.contains("port", ignoreCase = true) == true)
      }
    }
  }

  @Test
  fun `mede tamanho utf8 e aplica limite de 1024 bytes`() {
    assertEquals(8, Utf8DatagramSizer.byteLength("ae漢字"))
    assertTrue(Utf8DatagramSizer.fitsDatagram("a".repeat(1024)))
    assertFalse(Utf8DatagramSizer.fitsDatagram("a".repeat(1025)))
  }

  @Test
  fun `close e idempotente e nao permite envio depois do fechamento`() {
    val transport = UdpSocketTransport(
      socketFactory = { DatagramSocket() },
    )

    transport.open(UdpDestination.parse(host = "127.0.0.1", port = 57421))
    transport.close()
    transport.close()

    try {
      transport.send("payload")
      throw AssertionError("expected send after close to fail")
    } catch (error: IllegalStateException) {
      assertTrue(error.message?.contains("open", ignoreCase = true) == true)
    }
  }

  @Test
  fun `envio loopback por udp preserva o payload exato`() {
    val server = DatagramSocket(0, InetAddress.getByName("127.0.0.1"))

    try {
      val latch = CountDownLatch(1)
      val received = arrayOfNulls<ByteArray>(1)
      val listener = thread(start = true) {
        val buffer = ByteArray(1024)
        val packet = DatagramPacket(buffer, buffer.size)
        server.receive(packet)
        received[0] = packet.data.copyOf(packet.length)
        latch.countDown()
      }

      val transport = UdpSocketTransport(
        socketFactory = { DatagramSocket() },
      )
      val payload = "{\"kind\":\"motion_sample\",\"sessionId\":\"abc\"}"

      transport.open(UdpDestination.parse(host = "127.0.0.1", port = server.localPort))
      transport.send(payload)

      assertTrue(latch.await(2, TimeUnit.SECONDS))
      assertArrayEquals(payload.toByteArray(Charsets.UTF_8), received[0])

      transport.close()
      listener.join(2000)
    } finally {
      server.close()
    }
  }

  @Test
  fun `loopback 127 slash 8 permanece permitido para testes locais`() {
    val destination = UdpDestination.parse(host = "127.12.34.56", port = 57421)

    assertEquals("127.12.34.56", destination.hostAddress)
  }
}
