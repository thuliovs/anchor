package com.anchormobile.motion

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class HostLifecycleAdmissionTest {
  @Test
  fun `start e rejeitado enquanto host estiver pausado`() {
    val admission = HostLifecycleAdmission()

    admission.onHostPause()

    val result = admission.tryStart()

    assertTrue(result is HostStartAdmission.Rejected)
    val rejected = result as HostStartAdmission.Rejected
    assertEquals("ERR_HOST_INACTIVE", rejected.code)
  }

  @Test
  fun `host resume apenas libera novo start manual`() {
    val admission = HostLifecycleAdmission()

    admission.onHostPause()
    admission.onHostResume()

    val result = admission.tryStart()

    assertTrue(result is HostStartAdmission.Allowed)
  }

  @Test
  fun `host destroy mantem start bloqueado`() {
    val admission = HostLifecycleAdmission()

    admission.onHostDestroy()

    val result = admission.tryStart()

    assertTrue(result is HostStartAdmission.Rejected)
    val rejected = result as HostStartAdmission.Rejected
    assertEquals("ERR_HOST_INACTIVE", rejected.code)
  }
}
