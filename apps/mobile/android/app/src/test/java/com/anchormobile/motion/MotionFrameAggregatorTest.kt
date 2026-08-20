package com.anchormobile.motion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MotionFrameAggregatorTest {
  @Test
  fun `nenhum frame antes dos tres sensores estarem prontos`() {
    val aggregator = MotionFrameAggregator()
    startSession(aggregator)

    aggregator.onLinearAcceleration(1_000_000, 1f, 2f, 3f)
    aggregator.onGravity(2_000_000, 0f, 0f, 9.80665f)

    val frame = aggregator.onGyroscope(3_000_000, 0.1f, 0.2f, 0.3f)

    assertEquals(-9.80665, frame!!.gravityMps2.z, 0.0001)
  }

  @Test
  fun `aceleracao linear preserva eixos e gravidade inverte sinal`() {
    val aggregator = MotionFrameAggregator()
    startSession(aggregator)

    aggregator.onLinearAcceleration(1_000_000, 4f, -5f, 6f)
    aggregator.onGravity(2_000_000, 0f, 0f, 9.80665f)
    val frame = aggregator.onGyroscope(3_000_000, 0.5f, -0.25f, 0.75f)!!

    assertEquals(4.0, frame.linearAccelerationMps2.x, 0.0)
    assertEquals(-5.0, frame.linearAccelerationMps2.y, 0.0)
    assertEquals(6.0, frame.linearAccelerationMps2.z, 0.0)
    assertEquals(0.5, frame.angularVelocityRadS.x, 0.0)
    assertEquals(-0.25, frame.angularVelocityRadS.y, 0.0)
    assertEquals(0.75, frame.angularVelocityRadS.z, 0.0)
    assertEquals(-9.80665, frame.gravityMps2.z, 0.0001)
  }

  @Test
  fun `limitacao fica no maximo em 60 hz e nao gera rajada apos atraso`() {
    val aggregator = MotionFrameAggregator()
    startSession(aggregator)

    aggregator.onLinearAcceleration(1_000_000, 0f, 0f, 0f)
    aggregator.onGravity(1_000_000, 0f, 0f, 9.80665f)

    val first = aggregator.onGyroscope(1_000_000, 0f, 0f, 0f)
    val skipped = aggregator.onGyroscope(10_000_000, 0f, 0f, 0f)
    val delayed = aggregator.onGyroscope(40_000_000, 0f, 0f, 0f)
    val delayedImmediate = aggregator.onGyroscope(40_100_000, 0f, 0f, 0f)

    assertEquals(0L, first!!.sequence)
    assertNull(skipped)
    assertEquals(1L, delayed!!.sequence)
    assertNull(delayedImmediate)
  }

  @Test
  fun `sequencia e tempo monotonic reiniciam em nova sessao`() {
    val aggregator = MotionFrameAggregator()
    startSession(aggregator, sessionId = "session-a")

    aggregator.onLinearAcceleration(1_000_000, 0f, 0f, 0f)
    aggregator.onGravity(1_000_000, 0f, 0f, 9.80665f)
    val firstFrame = aggregator.onGyroscope(1_000_000, 0f, 0f, 0f)!!

    aggregator.stop()
    startSession(aggregator, sessionId = "session-b", startNs = 5_000_000)
    aggregator.onLinearAcceleration(6_000_000, 0f, 0f, 0f)
    aggregator.onGravity(6_000_000, 0f, 0f, 9.80665f)
    val secondFrame = aggregator.onGyroscope(6_000_000, 0f, 0f, 0f)!!

    assertEquals(0L, firstFrame.sequence)
    assertEquals(0L, firstFrame.sessionElapsedUs)
    assertEquals("session-b", secondFrame.sessionId)
    assertEquals(0L, secondFrame.sequence)
    assertEquals(1_000L, secondFrame.sessionElapsedUs)
  }

  @Test
  fun `valores nao finitos nao sao emitidos e ultima leitura nao cria fila ilimitada`() {
    val aggregator = MotionFrameAggregator()
    startSession(aggregator)

    aggregator.onLinearAcceleration(1_000_000, 1f, 1f, 1f)
    aggregator.onLinearAcceleration(1_100_000, 2f, 2f, 2f)
    aggregator.onGravity(1_200_000, 0f, 0f, 9.80665f)

    val invalid = aggregator.onGyroscope(1_300_000, Float.NaN, 0f, 0f)
    val valid = aggregator.onGyroscope(2_000_000, 0f, 0f, 0f)

    assertNull(invalid)
    assertEquals(2.0, valid!!.linearAccelerationMps2.x, 0.0)
  }

  @Test
  fun `sensores ausentes impedem inicio parcial`() {
    val aggregator = MotionFrameAggregator()
    val result = aggregator.startSession(
      StartSessionRequest(
        sessionId = "session-a",
        requestedRateHz = 60.0,
        sessionStartTimestampNs = 0,
        availability = SensorAvailability(linearAcceleration = true, gravity = false, gyroscope = true),
      ),
    )

    assertTrue(result is StartSessionResult.MissingSensors)
    val missing = result as StartSessionResult.MissingSensors
    assertEquals(listOf("gravity"), missing.missingSensors)
  }

  private fun startSession(
    aggregator: MotionFrameAggregator,
    sessionId: String = "session-a",
    startNs: Long = 1_000_000,
  ) {
    aggregator.startSession(
      StartSessionRequest(
        sessionId = sessionId,
        requestedRateHz = 60.0,
        sessionStartTimestampNs = startNs,
        availability = SensorAvailability(true, true, true),
      ),
    )
  }
}
