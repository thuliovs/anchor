package com.anchormobile.motion

import java.net.DatagramSocket
import java.net.SocketException
import java.util.concurrent.CountDownLatch
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeUdpSenderModuleBehaviorTest {
  @Test
  fun `open obsoleto nao ressuscita socket depois de close`() {
    val previousCloseEntered = CountDownLatch(1)
    val allowPreviousCloseToFinish = CountDownLatch(1)
    val obsoleteSocketClosed = CountDownLatch(1)
    val installedSocket = ControlledDatagramSocket(
      onClose = {
        previousCloseEntered.countDown()
        assertTrue(allowPreviousCloseToFinish.await(2, TimeUnit.SECONDS))
      },
    )
    val createdSockets = mutableListOf<ControlledDatagramSocket>()
    val transport = UdpSocketTransport(
      socketFactory = {
        ControlledDatagramSocket(
          onClose = {
            obsoleteSocketClosed.countDown()
          },
        ).also(createdSockets::add)
      },
    )

    transport.installForTest(UdpDestination.parse("127.0.0.1", 57421), installedSocket)

    val openFinished = CountDownLatch(1)
    val openThread = Thread {
      transport.open(UdpDestination.parse("127.0.0.1", 57422))
      openFinished.countDown()
    }
    openThread.start()

    assertTrue(previousCloseEntered.await(2, TimeUnit.SECONDS))

    transport.close()
    allowPreviousCloseToFinish.countDown()

    assertTrue(openFinished.await(2, TimeUnit.SECONDS))
    openThread.join(2000)

    assertEquals(1, createdSockets.size)
    assertTrue(obsoleteSocketClosed.await(2, TimeUnit.SECONDS))
    assertTrue(createdSockets[0].wasClosed())
    assertNull(transport.currentDestinationForTest())

    try {
      transport.send("payload")
      throw AssertionError("expected transport to remain closed")
    } catch (error: IllegalStateException) {
      assertTrue(error.message?.contains("not open", ignoreCase = true) == true)
    }
  }

  @Test
  fun `close pode acontecer enquanto send esta bloqueado sem esperar indefinidamente`() {
    val sendEntered = CountDownLatch(1)
    val releaseSend = CountDownLatch(1)
    val socketClosed = CountDownLatch(1)
    val sendFinished = CountDownLatch(1)
    val transport = UdpSocketTransport(
      socketFactory = {
        object : DatagramSocket() {
          override fun send(packet: java.net.DatagramPacket) {
            sendEntered.countDown()
            releaseSend.await(2, TimeUnit.SECONDS)
            sendFinished.countDown()
          }

          override fun close() {
            super.close()
            socketClosed.countDown()
            releaseSend.countDown()
          }
        }
      },
    )

    transport.open(UdpDestination.parse("127.0.0.1", 57421))

    val sendThread = Thread {
      try {
        transport.send("payload")
      } catch (_: Exception) {
      }
    }
    sendThread.start()

    assertTrue(sendEntered.await(2, TimeUnit.SECONDS))

    val closeReturned = CountDownLatch(1)
    Thread {
      transport.close()
      closeReturned.countDown()
    }.start()

    assertTrue(closeReturned.await(2, TimeUnit.SECONDS))
    assertTrue(socketClosed.await(2, TimeUnit.SECONDS))
    assertTrue(sendFinished.await(2, TimeUnit.SECONDS))
    sendThread.join(2000)
  }

  @Test
  fun `open que falha rejeita exatamente uma vez e restaura estado fechado`() {
    val controller = NativeUdpSenderController(
      transportFactory = {
        UdpSocketTransport(socketFactory = { throw SocketException("boom") })
      },
      workerFactory = { Executors.newSingleThreadExecutor() },
    )
    val promise = RecordingPromise()

    controller.open("192.168.0.20", 57421, promise)

    promise.awaitTerminalEvent()

    assertEquals(1, promise.rejectCount)
    assertEquals(NativeUdpSenderController.ERR_OPEN_FAILED, promise.lastRejectCode)
    assertEquals(false, controller.isTransportOpen())
  }

  @Test
  fun `close encerra worker da sessao`() {
    val createdWorkers = mutableListOf<TrackingExecutorService>()
    val controller = NativeUdpSenderController(
      transportFactory = { UdpSocketTransport(socketFactory = { DatagramSocket() }) },
      workerFactory = {
        TrackingExecutorService().also { createdWorkers.add(it) }
      },
    )

    val firstOpen = RecordingPromise()
    controller.open("192.168.0.20", 57421, firstOpen)
    firstOpen.awaitTerminalEvent()
    assertTrue(controller.isTransportOpen())

    controller.close()
    assertTrue(createdWorkers[0].shutdownCalled)
    assertTrue(createdWorkers[0].isShutdown)
  }

  @Test
  fun `pause encerra worker da sessao e resume permite nova abertura manual sem reabrir automaticamente`() {
    val createdWorkers = mutableListOf<TrackingExecutorService>()
    val controller = NativeUdpSenderController(
      transportFactory = { UdpSocketTransport(socketFactory = { DatagramSocket() }) },
      workerFactory = {
        TrackingExecutorService().also { createdWorkers.add(it) }
      },
    )

    val firstOpen = RecordingPromise()
    controller.open("192.168.0.20", 57421, firstOpen)
    firstOpen.awaitTerminalEvent()

    controller.onHostPause()
    assertTrue(createdWorkers[0].shutdownCalled)
    assertTrue(createdWorkers[0].isShutdown)

    controller.onHostResume()
    assertFalse(controller.isTransportOpen())

    val pausedOpen = RecordingPromise()
    controller.open("192.168.0.21", 57421, pausedOpen)
    pausedOpen.awaitTerminalEvent()
    assertEquals(1, pausedOpen.resolveCount)
    assertEquals(2, createdWorkers.size)
  }

  @Test
  fun `destroy encerra worker da sessao`() {
    val createdWorkers = mutableListOf<TrackingExecutorService>()
    val controller = NativeUdpSenderController(
      transportFactory = { UdpSocketTransport(socketFactory = { DatagramSocket() }) },
      workerFactory = {
        TrackingExecutorService().also { createdWorkers.add(it) }
      },
    )

    val firstOpen = RecordingPromise()
    controller.open("192.168.0.20", 57421, firstOpen)
    firstOpen.awaitTerminalEvent()

    controller.onHostDestroy()
    assertTrue(createdWorkers[0].shutdownCalled)
    assertTrue(createdWorkers[0].isShutdown)
  }

  @Test
  fun `invalidate encerra worker da sessao`() {
    val createdWorkers = mutableListOf<TrackingExecutorService>()
    val controller = NativeUdpSenderController(
      transportFactory = { UdpSocketTransport(socketFactory = { DatagramSocket() }) },
      workerFactory = {
        TrackingExecutorService().also { createdWorkers.add(it) }
      },
    )

    val firstOpen = RecordingPromise()
    controller.open("192.168.0.20", 57421, firstOpen)
    firstOpen.awaitTerminalEvent()

    controller.invalidate()
    assertTrue(createdWorkers[0].shutdownCalled)
    assertTrue(createdWorkers[0].isShutdown)
  }

  @Test
  fun `port normalizer aceita e rejeita valores esperados`() {
    assertEquals(1, NativeUdpSenderPortNormalizer.normalize(1.0))
    assertEquals(65535, NativeUdpSenderPortNormalizer.normalize(65535.0))

    for (invalid in listOf(Double.NaN, Double.POSITIVE_INFINITY, 1.5, 0.0, -1.0, 65536.0)) {
      try {
        NativeUdpSenderPortNormalizer.normalize(invalid)
        throw AssertionError("expected invalid port $invalid")
      } catch (error: IllegalArgumentException) {
        assertTrue(error.message?.contains("port", ignoreCase = true) == true)
      }
    }
  }
}

private class ControlledDatagramSocket(
  private val onClose: (() -> Unit)? = null,
) : DatagramSocket() {
  private var closedByTest = false

  override fun close() {
    onClose?.invoke()
    closedByTest = true
    super.close()
  }

  fun wasClosed(): Boolean = closedByTest
}

private class RecordingPromise : PromiseSink {
  private val latch = CountDownLatch(1)
  var resolveCount = 0
    private set
  var rejectCount = 0
    private set
  var lastRejectCode: String? = null
    private set
  var lastRejectMessage: String? = null
    private set

  override fun resolve(value: Any?) {
    resolveCount += 1
    latch.countDown()
  }

  override fun reject(code: String, message: String?) {
    rejectCount += 1
    lastRejectCode = code
    lastRejectMessage = message
    latch.countDown()
  }

  fun awaitTerminalEvent() {
    assertTrue(latch.await(2, TimeUnit.SECONDS))
  }
}

private class TrackingExecutorService : ExecutorService {
  private val delegate = Executors.newSingleThreadExecutor()
  var shutdownCalled = false
    private set

  override fun execute(command: Runnable) {
    delegate.execute(command)
  }

  override fun <T : Any?> submit(task: java.util.concurrent.Callable<T>): java.util.concurrent.Future<T> =
    delegate.submit(task)

  override fun <T : Any?> submit(task: Runnable, result: T): java.util.concurrent.Future<T> =
    delegate.submit(task, result)

  override fun submit(task: Runnable): java.util.concurrent.Future<*> = delegate.submit(task)

  override fun shutdown() {
    shutdownCalled = true
    delegate.shutdown()
  }

  override fun shutdownNow(): MutableList<Runnable> {
    shutdownCalled = true
    return delegate.shutdownNow()
  }

  override fun isShutdown(): Boolean = delegate.isShutdown

  override fun isTerminated(): Boolean = delegate.isTerminated

  override fun awaitTermination(timeout: Long, unit: TimeUnit): Boolean =
    delegate.awaitTermination(timeout, unit)

  override fun <T : Any?> invokeAll(tasks: MutableCollection<out java.util.concurrent.Callable<T>>): MutableList<java.util.concurrent.Future<T>> =
    delegate.invokeAll(tasks)

  override fun <T : Any?> invokeAll(
    tasks: MutableCollection<out java.util.concurrent.Callable<T>>,
    timeout: Long,
    unit: TimeUnit,
  ): MutableList<java.util.concurrent.Future<T>> = delegate.invokeAll(tasks, timeout, unit)

  override fun <T : Any?> invokeAny(tasks: MutableCollection<out java.util.concurrent.Callable<T>>): T =
    delegate.invokeAny(tasks)

  override fun <T : Any?> invokeAny(
    tasks: MutableCollection<out java.util.concurrent.Callable<T>>,
    timeout: Long,
    unit: TimeUnit,
  ): T = delegate.invokeAny(tasks, timeout, unit)
}
