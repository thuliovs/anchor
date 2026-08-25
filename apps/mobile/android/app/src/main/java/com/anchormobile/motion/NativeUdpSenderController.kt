package com.anchormobile.motion

import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean

internal interface PromiseSink {
  fun resolve(value: Any? = null)

  fun reject(code: String, message: String?)
}

internal class NativeUdpSenderController(
  private val transportFactory: () -> UdpSocketTransport = { UdpSocketTransport() },
  private val workerFactory: () -> ExecutorService = {
    Executors.newSingleThreadExecutor { runnable ->
      Thread(runnable, "AnchorUdpSender").apply { isDaemon = true }
    }
  },
) {
  private val lock = Any()
  private var hostActive = true
  private var generation = 0L
  private var worker: ExecutorService? = null
  private var transport: UdpSocketTransport? = null
  private var isOpen = false
  private val pendingPromises = linkedSetOf<SettlingPromise>()

  fun open(host: String, port: Int, promise: PromiseSink) {
    val destination = try {
      UdpDestination.parse(host, port)
    } catch (error: IllegalArgumentException) {
      promise.reject(ERR_INVALID_DESTINATION, error.message)
      return
    }

    val settlingPromise = SettlingPromise(promise)
    val submission = synchronized(lock) {
      if (!hostActive) {
        settlingPromise.reject(ERR_HOST_INACTIVE, "Host is not active; manual open is unavailable while paused.")
        return
      }

      val cleanup = closeSessionLocked(ERR_NOT_OPEN, "UDP transport is not open.")
      val newWorker = workerFactory()
      val newTransport = transportFactory()
      generation += 1
      val currentGeneration = generation
      worker = newWorker
      transport = newTransport
      isOpen = false
      pendingPromises.add(settlingPromise)
      OpenSubmission(cleanup, currentGeneration, newWorker, newTransport, settlingPromise, destination)
    }

    cleanupSession(submission.cleanup)
    execute(submission.worker, submission.promise) {
      try {
        submission.transport.open(submission.destination)
      } catch (error: Exception) {
        failOpen(submission, error)
        return@execute
      }

      val shouldResolve = synchronized(lock) {
        val current = generation == submission.generation && hostActive && transport === submission.transport
        if (current) {
          isOpen = true
        }
        pendingPromises.remove(submission.promise)
        current
      }

      if (shouldResolve) {
        submission.promise.resolve()
      } else {
        submission.transport.close()
        submission.promise.reject(ERR_NOT_OPEN, "UDP transport is not open.")
      }
    }
  }

  fun send(payload: String, promise: PromiseSink) {
    if (payload.isEmpty()) {
      promise.reject(ERR_SEND_FAILED, "UDP payload must not be empty.")
      return
    }

    val byteLength = Utf8DatagramSizer.byteLength(payload)
    if (byteLength > Utf8DatagramSizer.MAX_DATAGRAM_BYTES) {
      promise.reject(ERR_PAYLOAD_TOO_LARGE, "UDP payload exceeds ${Utf8DatagramSizer.MAX_DATAGRAM_BYTES} bytes.")
      return
    }

    val settlingPromise = SettlingPromise(promise)
    val submission = synchronized(lock) {
      if (!hostActive) {
        settlingPromise.reject(ERR_HOST_INACTIVE, "Host is not active; UDP send is unavailable while paused.")
        return
      }

      val currentWorker = worker
      val currentTransport = transport
      if (!isOpen || currentWorker == null || currentTransport == null) {
        settlingPromise.reject(ERR_NOT_OPEN, "UDP transport is not open.")
        return
      }

      pendingPromises.add(settlingPromise)
      SendSubmission(generation, currentWorker, currentTransport, settlingPromise, payload)
    }

    execute(submission.worker, submission.promise) {
      try {
        submission.transport.send(submission.payload)
      } catch (error: IllegalArgumentException) {
        synchronized(lock) {
          pendingPromises.remove(submission.promise)
        }
        submission.promise.reject(ERR_PAYLOAD_TOO_LARGE, error.message)
        return@execute
      } catch (error: IllegalStateException) {
        synchronized(lock) {
          pendingPromises.remove(submission.promise)
        }
        submission.promise.reject(ERR_NOT_OPEN, error.message)
        return@execute
      } catch (error: Exception) {
        synchronized(lock) {
          pendingPromises.remove(submission.promise)
        }
        submission.promise.reject(ERR_SEND_FAILED, error.message)
        return@execute
      }

      val shouldResolve = synchronized(lock) {
        val current = generation == submission.generation && transport === submission.transport && isOpen
        pendingPromises.remove(submission.promise)
        current
      }

      if (shouldResolve) {
        submission.promise.resolve()
      } else {
        submission.promise.reject(ERR_NOT_OPEN, "UDP transport is not open.")
      }
    }
  }

  fun close() {
    cleanupSession(synchronized(lock) { closeSessionLocked(ERR_NOT_OPEN, "UDP transport is not open.") })
  }

  fun onHostResume() {
    synchronized(lock) {
      hostActive = true
    }
  }

  fun onHostPause() {
    val cleanup = synchronized(lock) {
      hostActive = false
      closeSessionLocked(ERR_HOST_INACTIVE, "Host is not active; UDP transport is unavailable while paused.")
    }
    cleanupSession(cleanup)
  }

  fun onHostDestroy() {
    val cleanup = synchronized(lock) {
      hostActive = false
      closeSessionLocked(ERR_HOST_INACTIVE, "Host is not active; UDP transport is unavailable while destroyed.")
    }
    cleanupSession(cleanup)
  }

  fun invalidate() {
    val cleanup = synchronized(lock) {
      hostActive = false
      closeSessionLocked(ERR_HOST_INACTIVE, "Host is not active; UDP transport is unavailable.")
    }
    cleanupSession(cleanup)
  }

  fun isTransportOpen(): Boolean = synchronized(lock) { isOpen }

  private fun failOpen(submission: OpenSubmission, error: Exception) {
    val cleanup = synchronized(lock) {
      if (transport !== submission.transport || generation != submission.generation) {
        pendingPromises.remove(submission.promise)
        null
      } else {
        pendingPromises.remove(submission.promise)
        isOpen = false
        generation += 1
        val detachedWorker = worker
        val detachedTransport = transport
        worker = null
        transport = null
        SessionCleanup(detachedTransport, detachedWorker)
      }
    }

    cleanupSession(cleanup)
    submission.promise.reject(ERR_OPEN_FAILED, error.message)
  }

  private fun execute(worker: ExecutorService, promise: SettlingPromise, action: () -> Unit) {
    try {
      worker.execute(action)
    } catch (_: RejectedExecutionException) {
      synchronized(lock) {
        pendingPromises.remove(promise)
      }
      promise.reject(ERR_HOST_INACTIVE, "UDP worker is unavailable.")
    }
  }

  private fun closeSessionLocked(rejectCode: String, rejectMessage: String): SessionCleanup {
    generation += 1
    isOpen = false
    val detachedTransport = transport
    val detachedWorker = worker
    val promises = pendingPromises.toList()
    transport = null
    worker = null
    pendingPromises.clear()

    promises.forEach { it.reject(rejectCode, rejectMessage) }

    return SessionCleanup(detachedTransport, detachedWorker)
  }

  private fun cleanupSession(cleanup: SessionCleanup?) {
    if (cleanup == null) {
      return
    }

    cleanup.transport?.close()
    cleanup.worker?.shutdownNow()
  }

  private data class OpenSubmission(
    val cleanup: SessionCleanup,
    val generation: Long,
    val worker: ExecutorService,
    val transport: UdpSocketTransport,
    val promise: SettlingPromise,
    val destination: UdpDestination,
  )

  private data class SendSubmission(
    val generation: Long,
    val worker: ExecutorService,
    val transport: UdpSocketTransport,
    val promise: SettlingPromise,
    val payload: String,
  )

  private data class SessionCleanup(
    val transport: UdpSocketTransport?,
    val worker: ExecutorService?,
  )

  companion object {
    const val ERR_INVALID_DESTINATION = "ERR_INVALID_DESTINATION"
    const val ERR_NOT_OPEN = "ERR_NOT_OPEN"
    const val ERR_PAYLOAD_TOO_LARGE = "ERR_PAYLOAD_TOO_LARGE"
    const val ERR_SEND_FAILED = "ERR_SEND_FAILED"
    const val ERR_HOST_INACTIVE = "ERR_HOST_INACTIVE"
    const val ERR_OPEN_FAILED = "ERR_OPEN_FAILED"
  }
}

internal class SettlingPromise(
  private val sink: PromiseSink,
) {
  private val settled = AtomicBoolean(false)

  fun resolve(value: Any? = null) {
    if (settled.compareAndSet(false, true)) {
      sink.resolve(value)
    }
  }

  fun reject(code: String, message: String?) {
    if (settled.compareAndSet(false, true)) {
      sink.reject(code, message)
    }
  }
}
