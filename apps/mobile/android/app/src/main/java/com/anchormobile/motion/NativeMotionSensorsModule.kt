package com.anchormobile.motion

import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.LifecycleEventListener
import com.facebook.react.bridge.Promise
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.bridge.WritableArray
import com.facebook.react.bridge.WritableMap
import com.facebook.react.module.annotations.ReactModule
import com.facebook.fbreact.specs.NativeMotionSensorsSpec
import java.util.UUID

@ReactModule(name = NativeMotionSensorsModule.NAME)
class NativeMotionSensorsModule(
  reactContext: ReactApplicationContext,
) : NativeMotionSensorsSpec(reactContext), SensorEventListener, LifecycleEventListener {
  private val sensorManager = reactContext.getSystemService(SensorManager::class.java)
  private val lock = Any()
  private val aggregator = MotionFrameAggregator()
  private val hostLifecycleAdmission = HostLifecycleAdmission()

  private var handlerThread: HandlerThread? = null
  private var sensorHandler: Handler? = null
  private var activeSession: ActiveSession? = null

  init {
    reactApplicationContext.addLifecycleEventListener(this)
  }

  override fun getName(): String = NAME

  @ReactMethod
  override fun getAvailability(promise: Promise) {
    promise.resolve(createAvailabilityMap(readAvailability()))
  }

  @ReactMethod
  override fun start(rateHz: Double, promise: Promise) {
    val normalizedRateHz = normalizeRateHz(rateHz)
      ?: run {
        promise.reject("ERR_INVALID_RATE", "rateHz must be finite and positive")
        return
      }

    val sensors = resolveSensors()
    if (sensors.missingSensors.isNotEmpty()) {
      promise.reject(
        "ERR_SENSORS_UNAVAILABLE",
        "Required motion sensors are unavailable",
        createMissingSensorsDetails(sensors.missingSensors),
      )
      return
    }

    synchronized(lock) {
      when (val admission = hostLifecycleAdmission.tryStart()) {
        is HostStartAdmission.Rejected -> {
          promise.reject(admission.code, admission.message)
          return
        }
        HostStartAdmission.Allowed -> Unit
      }

      activeSession?.let { session ->
        promise.resolve(createStartResultMap(session.sessionId, session.requestedRateHz))
        return
      }

      val thread = HandlerThread("AnchorMotionSensors").apply { start() }
      val handler = Handler(thread.looper)
      val sessionId = UUID.randomUUID().toString()
      val startResult = aggregator.startSession(
        StartSessionRequest(
          sessionId = sessionId,
          requestedRateHz = normalizedRateHz,
          sessionStartTimestampNs = SystemClock.elapsedRealtimeNanos(),
          availability = SensorAvailability(true, true, true),
        ),
      )

      val started = startResult as? StartSessionResult.Started
        ?: run {
          thread.quitSafely()
          promise.reject("ERR_SENSORS_UNAVAILABLE", "Unable to start motion session")
          return
        }

      val samplingPeriodUs = maxOf(1, (1_000_000.0 / started.requestedRateHz).toInt())
      val registered = registerSensors(
        sensors = sensors,
        handler = handler,
        samplingPeriodUs = samplingPeriodUs,
      )

      if (!registered) {
        sensorManager.unregisterListener(this)
        aggregator.stop()
        thread.quitSafely()
        promise.reject("ERR_SENSOR_REGISTRATION", "Failed to register Android motion sensors")
        return
      }

      handlerThread = thread
      sensorHandler = handler
      activeSession = ActiveSession(started.sessionId, started.requestedRateHz)
      promise.resolve(createStartResultMap(started.sessionId, started.requestedRateHz))
    }
  }

  @ReactMethod
  override fun stop() {
    synchronized(lock) {
      stopInternal()
    }
  }

  override fun onSensorChanged(event: SensorEvent) {
    val frame = synchronized(lock) {
      if (activeSession == null) {
        null
      } else {
        when (event.sensor.type) {
          Sensor.TYPE_LINEAR_ACCELERATION -> aggregator.onLinearAcceleration(
            event.timestamp,
            event.values[0],
            event.values[1],
            event.values[2],
          )
          Sensor.TYPE_GRAVITY -> aggregator.onGravity(
            event.timestamp,
            event.values[0],
            event.values[1],
            event.values[2],
          )
          Sensor.TYPE_GYROSCOPE -> aggregator.onGyroscope(
            event.timestamp,
            event.values[0],
            event.values[1],
            event.values[2],
          )
          else -> null
        }
      }
    }

    if (frame != null) {
      emitOnMotionFrame(createFrameMap(frame))
    }
  }

  override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit

  override fun onHostResume() {
    synchronized(lock) {
      hostLifecycleAdmission.onHostResume()
    }
  }

  override fun onHostPause() {
    synchronized(lock) {
      hostLifecycleAdmission.onHostPause()
      stopInternal()
    }
  }

  override fun onHostDestroy() {
    synchronized(lock) {
      hostLifecycleAdmission.onHostDestroy()
      stopInternal()
    }
  }

  override fun invalidate() {
    synchronized(lock) {
      hostLifecycleAdmission.onHostDestroy()
      stopInternal()
    }
    reactApplicationContext.removeLifecycleEventListener(this)
    super.invalidate()
  }

  private fun stopInternal() {
    sensorManager.unregisterListener(this)
    sensorHandler?.removeCallbacksAndMessages(null)
    handlerThread?.quitSafely()
    sensorHandler = null
    handlerThread = null
    activeSession = null
    aggregator.stop()
  }

  private fun registerSensors(
    sensors: ResolvedSensors,
    handler: Handler,
    samplingPeriodUs: Int,
  ): Boolean {
    val linearRegistered = sensorManager.registerListener(
      this,
      sensors.linearAcceleration,
      samplingPeriodUs,
      handler,
    )
    if (!linearRegistered) {
      return false
    }

    val gravityRegistered = sensorManager.registerListener(
      this,
      sensors.gravity,
      samplingPeriodUs,
      handler,
    )
    if (!gravityRegistered) {
      return false
    }

    val gyroscopeRegistered = sensorManager.registerListener(
      this,
      sensors.gyroscope,
      samplingPeriodUs,
      handler,
    )
    return gyroscopeRegistered
  }

  private fun resolveSensors(): ResolvedSensors {
    val linearAcceleration = sensorManager.getDefaultSensor(Sensor.TYPE_LINEAR_ACCELERATION)
    val gravity = sensorManager.getDefaultSensor(Sensor.TYPE_GRAVITY)
    val gyroscope = sensorManager.getDefaultSensor(Sensor.TYPE_GYROSCOPE)
    val missingSensors = buildList {
      if (linearAcceleration == null) add("linearAcceleration")
      if (gravity == null) add("gravity")
      if (gyroscope == null) add("gyroscope")
    }

    return ResolvedSensors(linearAcceleration, gravity, gyroscope, missingSensors)
  }

  private fun readAvailability(): SensorAvailability {
    val sensors = resolveSensors()
    return SensorAvailability(
      linearAcceleration = sensors.linearAcceleration != null,
      gravity = sensors.gravity != null,
      gyroscope = sensors.gyroscope != null,
    )
  }

  private fun createAvailabilityMap(availability: SensorAvailability): WritableMap =
    Arguments.createMap().apply {
      putBoolean("linearAcceleration", availability.linearAcceleration)
      putBoolean("gravity", availability.gravity)
      putBoolean("gyroscope", availability.gyroscope)
    }

  private fun createStartResultMap(sessionId: String, requestedRateHz: Double): WritableMap =
    Arguments.createMap().apply {
      putString("sessionId", sessionId)
      putDouble("requestedRateHz", requestedRateHz)
    }

  private fun createMissingSensorsDetails(missingSensors: List<String>): WritableMap =
    Arguments.createMap().apply {
      putArray("missingSensors", createStringArray(missingSensors))
    }

  private fun createFrameMap(frame: NativeMotionFramePayload): WritableMap =
    Arguments.createMap().apply {
      putString("sessionId", frame.sessionId)
      putDouble("sequence", frame.sequence.toDouble())
      putDouble("sessionElapsedUs", frame.sessionElapsedUs.toDouble())
      putMap("linearAccelerationMps2", createVectorMap(frame.linearAccelerationMps2))
      putMap("gravityMps2", createVectorMap(frame.gravityMps2))
      putMap("angularVelocityRadS", createVectorMap(frame.angularVelocityRadS))
    }

  private fun createVectorMap(vector: MotionVector): WritableMap = Arguments.createMap().apply {
    putDouble("x", vector.x)
    putDouble("y", vector.y)
    putDouble("z", vector.z)
  }

  private fun createStringArray(values: List<String>): WritableArray = Arguments.createArray().apply {
    values.forEach { pushString(it) }
  }

  private fun normalizeRateHz(rateHz: Double): Double? {
    if (!rateHz.isFinite() || rateHz <= 0) {
      return null
    }

    return minOf(rateHz, 60.0)
  }

  private data class ActiveSession(
    val sessionId: String,
    val requestedRateHz: Double,
  )

  private data class ResolvedSensors(
    val linearAcceleration: Sensor?,
    val gravity: Sensor?,
    val gyroscope: Sensor?,
    val missingSensors: List<String>,
  )

  companion object {
    const val NAME = "NativeMotionSensors"
  }
}
