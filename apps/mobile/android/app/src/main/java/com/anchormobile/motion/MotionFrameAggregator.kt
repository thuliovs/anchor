package com.anchormobile.motion

import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToLong

data class SensorAvailability(
  val linearAcceleration: Boolean,
  val gravity: Boolean,
  val gyroscope: Boolean,
) {
  fun missingSensors(): List<String> = buildList {
    if (!linearAcceleration) add("linearAcceleration")
    if (!gravity) add("gravity")
    if (!gyroscope) add("gyroscope")
  }
}

data class MotionVector(val x: Double, val y: Double, val z: Double)

data class NativeMotionFramePayload(
  val sessionId: String,
  val sequence: Long,
  val sessionElapsedUs: Long,
  val linearAccelerationMps2: MotionVector,
  val gravityMps2: MotionVector,
  val angularVelocityRadS: MotionVector,
)

data class StartSessionRequest(
  val sessionId: String,
  val requestedRateHz: Double,
  val sessionStartTimestampNs: Long,
  val availability: SensorAvailability,
)

sealed interface StartSessionResult {
  data class Started(
    val sessionId: String,
    val requestedRateHz: Double,
  ) : StartSessionResult

  data class AlreadyActive(
    val sessionId: String,
    val requestedRateHz: Double,
  ) : StartSessionResult

  data class MissingSensors(
    val missingSensors: List<String>,
  ) : StartSessionResult
}

class MotionFrameAggregator {
  private var activeSessionId: String? = null
  private var requestedRateHz: Double = 0.0
  private var sessionStartTimestampNs: Long = 0
  private var emitPeriodNs: Long = 0
  private var nextEmitTimestampNs: Long? = null
  private var nextSequence: Long = 0
  private var linearAcceleration: MotionVector? = null
  private var gravity: MotionVector? = null
  private var angularVelocity: MotionVector? = null

  fun startSession(request: StartSessionRequest): StartSessionResult {
    val missingSensors = request.availability.missingSensors()
    if (missingSensors.isNotEmpty()) {
      return StartSessionResult.MissingSensors(missingSensors)
    }

    if (activeSessionId != null) {
      return StartSessionResult.AlreadyActive(activeSessionId!!, requestedRateHz)
    }

    require(request.requestedRateHz.isFinite() && request.requestedRateHz > 0) {
      "requestedRateHz must be finite and positive"
    }

    activeSessionId = request.sessionId
    requestedRateHz = min(request.requestedRateHz, MAX_EMIT_RATE_HZ)
    sessionStartTimestampNs = request.sessionStartTimestampNs
    emitPeriodNs = max(1L, (1_000_000_000.0 / requestedRateHz).roundToLong())
    nextEmitTimestampNs = null
    nextSequence = 0
    linearAcceleration = null
    gravity = null
    angularVelocity = null

    return StartSessionResult.Started(request.sessionId, requestedRateHz)
  }

  fun stop() {
    activeSessionId = null
    requestedRateHz = 0.0
    sessionStartTimestampNs = 0
    emitPeriodNs = 0
    nextEmitTimestampNs = null
    nextSequence = 0
    linearAcceleration = null
    gravity = null
    angularVelocity = null
  }

  fun onLinearAcceleration(timestampNs: Long, x: Float, y: Float, z: Float): NativeMotionFramePayload? {
    linearAcceleration = toVector(x, y, z) ?: return null
    return null
  }

  fun onGravity(timestampNs: Long, x: Float, y: Float, z: Float): NativeMotionFramePayload? {
    gravity = toVector(-x, -y, -z) ?: return null
    return null
  }

  fun onGyroscope(timestampNs: Long, x: Float, y: Float, z: Float): NativeMotionFramePayload? {
    angularVelocity = toVector(x, y, z) ?: return null
    return emitIfReady(timestampNs)
  }

  private fun emitIfReady(timestampNs: Long): NativeMotionFramePayload? {
    val sessionId = activeSessionId ?: return null
    val linear = linearAcceleration ?: return null
    val gravityVector = gravity ?: return null
    val angular = angularVelocity ?: return null

    if (timestampNs < sessionStartTimestampNs || nextSequence > MAX_SEQUENCE) {
      return null
    }

    val scheduledEmitAt = nextEmitTimestampNs
    if (scheduledEmitAt != null && timestampNs < scheduledEmitAt) {
      return null
    }

    val frame = NativeMotionFramePayload(
      sessionId = sessionId,
      sequence = nextSequence,
      sessionElapsedUs = (timestampNs - sessionStartTimestampNs) / 1_000,
      linearAccelerationMps2 = linear,
      gravityMps2 = gravityVector,
      angularVelocityRadS = angular,
    )

    nextSequence += 1
    nextEmitTimestampNs = if (scheduledEmitAt == null) {
      timestampNs + emitPeriodNs
    } else {
      val skippedSlots = ((timestampNs - scheduledEmitAt) / emitPeriodNs) + 1
      scheduledEmitAt + (skippedSlots * emitPeriodNs)
    }

    return frame
  }

  private fun toVector(x: Float, y: Float, z: Float): MotionVector? {
    if (!x.isFinite() || !y.isFinite() || !z.isFinite()) {
      return null
    }

    return MotionVector(x.toDouble(), y.toDouble(), z.toDouble())
  }

  companion object {
    private const val MAX_EMIT_RATE_HZ = 60.0
    private const val MAX_SEQUENCE = 4_294_967_295L
  }
}
