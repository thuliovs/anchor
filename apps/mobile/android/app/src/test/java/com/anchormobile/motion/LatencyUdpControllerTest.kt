package com.anchormobile.motion

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

class LatencyUdpControllerTest {
  private class Promise : PromiseSink {
    val done = CountDownLatch(1)
    var error: String? = null
    override fun resolve(value: Any?) { done.countDown() }
    override fun reject(code: String, message: String?) { error = "$code:$message"; done.countDown() }
  }

  @Test
  fun bidirectionalSocketAnnouncesRespondsAndCloses() {
    val desktop = DatagramSocket(0, InetAddress.getByName("127.0.0.1")).apply { soTimeout = 2_000 }
    var now = 1_000L
    val controller = LatencyUdpController(monotonicUs = { now++ })
    val opened = Promise()
    controller.open("127.0.0.1", desktop.localPort, "measurement-test", opened)
    assertTrue(opened.done.await(2, TimeUnit.SECONDS))
    assertEquals(null, opened.error)
    val helloPacket = receive(desktop)
    val hello = JSONObject(String(helloPacket.data, helloPacket.offset, helloPacket.length))
    assertEquals("latency_hello_v1", hello.getString("kind"))

    val request = JSONObject().put("measurement_protocol_version", 1).put("kind", "clock_sync_request_v1")
      .put("measurement_id", "measurement-test").put("nonce", "a".repeat(32)).put("t1_desktop_us", 10)
      .toString().toByteArray()
    desktop.send(DatagramPacket(request, request.size, helloPacket.address, helloPacket.port))
    val responsePacket = receive(desktop)
    val response = JSONObject(String(responsePacket.data, responsePacket.offset, responsePacket.length, Charsets.UTF_8))
    assertEquals("clock_sync_response_v1", response.getString("kind"))
    assertTrue(response.getLong("t3_android_us") >= response.getLong("t2_android_us"))
    val sent=Promise()
    controller.sendSample("""{"sessionId":"session-a","sequence":7}""",900,sent)
    assertTrue(sent.done.await(1,TimeUnit.SECONDS))
    assertEquals(LatencyUdpController.Metrics(1,1,0,0),controller.metrics())
    val samplePacket=receive(desktop)
    assertEquals("latency_sample_v1",JSONObject(String(samplePacket.data,samplePacket.offset,samplePacket.length)).getString("kind"))
    controller.close()
    val rejected=Promise();controller.sendSample("{}",1,rejected)
    assertTrue(rejected.done.await(1,TimeUnit.SECONDS))
    assertEquals(LatencyUdpController.Metrics(2,1,1,0),controller.metrics())
    desktop.close()
  }

  @Test fun rejectsInvalidMeasurementId() { val p=Promise();LatencyUdpController().open("127.0.0.1",1,"",p);assertTrue(p.done.await(1,TimeUnit.SECONDS));assertTrue(p.error!!.startsWith("ERR_INVALID_MEASUREMENT_ID")) }

  private fun receive(socket: DatagramSocket): DatagramPacket {
    val bytes=ByteArray(LatencyUdpController.MAX_BYTES+1);val packet=DatagramPacket(bytes,bytes.size);socket.receive(packet);return packet
  }
}
