package com.anchormobile.motion

import android.os.SystemClock
import org.json.JSONObject
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.SocketException
import java.util.concurrent.Executors

internal class LatencyUdpController(
  private val socketFactory: () -> DatagramSocket = { DatagramSocket() },
  private val monotonicUs: () -> Long = { SystemClock.elapsedRealtimeNanos() / 1_000 },
) {
  internal data class Metrics(val offeredSamples:Long,val sentSamples:Long,val rejectedSamples:Long,val sendErrors:Long)
  private val lock = Any()
  private var socket: DatagramSocket? = null
  private var destination: UdpDestination? = null
  private var measurementId: String? = null
  private var generation = 0L
  private var offeredSamples=0L
  private var sentSamples=0L
  private var rejectedSamples=0L
  private var sendErrors=0L
  private var receiver = Executors.newSingleThreadExecutor { Thread(it, "AnchorLatencyUdpReceiver").apply { isDaemon = true } }

  fun open(host: String, port: Int, id: String, promise: PromiseSink) {
    if (id.isEmpty() || id.length > 128) { promise.reject("ERR_INVALID_MEASUREMENT_ID", "measurementId must contain 1..128 characters"); return }
    val target = try { UdpDestination.parse(host, port) } catch (e: IllegalArgumentException) { promise.reject("ERR_INVALID_DESTINATION", e.message); return }
    close()
    try {
      val opened = socketFactory()
      val current = synchronized(lock) { generation += 1; socket = opened; destination = target; measurementId = id; offeredSamples=0;sentSamples=0;rejectedSamples=0;sendErrors=0;generation }
      sendJson(opened, target, JSONObject().put("measurement_protocol_version", 1).put("kind", "latency_hello_v1").put("measurement_id", id))
      receiver = Executors.newSingleThreadExecutor { Thread(it, "AnchorLatencyUdpReceiver").apply { isDaemon = true } }
      receiver.execute { receiveLoop(current, opened, target, id) }
      promise.resolve()
    } catch (e: Exception) { close(); promise.reject("ERR_OPEN_FAILED", e.message) }
  }

  fun sendSample(payload: String, sensorUs: Long, promise: PromiseSink) {
    synchronized(lock){offeredSamples++}
    val state = synchronized(lock) { val s=socket;val d=destination;val id=measurementId;if(s==null||d==null||id==null)null else Triple(s,d,id) }
      ?: run { synchronized(lock){rejectedSamples++};promise.reject("ERR_NOT_OPEN", "Latency transport is not open"); return }
    try {
      val sample = JSONObject(payload)
      val preparationUs = monotonicUs()
      val packet = JSONObject().put("measurement_protocol_version",1).put("kind","latency_sample_v1").put("measurement_id",state.third)
        .put("android_sensor_event_us",sensorUs).put("android_send_preparation_us",preparationUs)
        .put("session_id",sample.getString("sessionId")).put("sequence",sample.getLong("sequence")).put("sample",sample)
      sendJson(state.first,state.second,packet);synchronized(lock){sentSamples++}; promise.resolve()
    } catch (e: Exception) { synchronized(lock){sendErrors++};promise.reject("ERR_SEND_FAILED",e.message) }
  }

  private fun receiveLoop(current: Long, opened: DatagramSocket, target: UdpDestination, id: String) {
    val data=ByteArray(MAX_BYTES+1)
    while (synchronized(lock){generation==current&&socket===opened}) try {
      val packet=DatagramPacket(data,data.size);opened.receive(packet);val t2=monotonicUs()
      if(packet.length>MAX_BYTES||packet.address!=target.inetAddress||packet.port!=target.port)continue
      val request=JSONObject(String(packet.data,packet.offset,packet.length,Charsets.UTF_8))
      val keys=request.keys().asSequence().toSet();if(keys!=setOf("measurement_protocol_version","kind","measurement_id","nonce","t1_desktop_us"))continue
      if(request.getInt("measurement_protocol_version")!=1||request.getString("kind")!="clock_sync_request_v1"||request.getString("measurement_id")!=id)continue
      val nonce=request.getString("nonce");if(nonce.length !in 32..128)continue
      val response=JSONObject().put("measurement_protocol_version",1).put("kind","clock_sync_response_v1").put("measurement_id",id).put("nonce",nonce)
        .put("t1_desktop_us",request.getLong("t1_desktop_us")).put("t2_android_us",t2)
      val t3=monotonicUs();response.put("t3_android_us",t3);sendJson(opened,target,response)
    } catch (_: SocketException) { break } catch (_: Exception) { /* invalid probe is ignored */ }
  }

  private fun sendJson(s:DatagramSocket,d:UdpDestination,json:JSONObject){val bytes=json.toString().toByteArray(Charsets.UTF_8);require(bytes.size<=MAX_BYTES);s.send(DatagramPacket(bytes,bytes.size,d.inetAddress,d.port))}
  internal fun metrics()=synchronized(lock){Metrics(offeredSamples,sentSamples,rejectedSamples,sendErrors)}
  fun close(){val old=synchronized(lock){generation+=1;val value=socket;socket=null;destination=null;measurementId=null;value};old?.close();receiver.shutdownNow()}
  companion object { const val MAX_BYTES=4096 }
}
