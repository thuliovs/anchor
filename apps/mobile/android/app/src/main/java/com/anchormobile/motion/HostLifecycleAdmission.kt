package com.anchormobile.motion

sealed interface HostStartAdmission {
  data object Allowed : HostStartAdmission

  data class Rejected(
    val code: String,
    val message: String,
  ) : HostStartAdmission
}

class HostLifecycleAdmission {
  private var hostActive = true

  fun onHostResume() {
    hostActive = true
  }

  fun onHostPause() {
    hostActive = false
  }

  fun onHostDestroy() {
    hostActive = false
  }

  fun tryStart(): HostStartAdmission {
    if (!hostActive) {
      return HostStartAdmission.Rejected(
        code = ERR_HOST_INACTIVE,
        message = "Host is not active; manual start is unavailable while paused.",
      )
    }

    return HostStartAdmission.Allowed
  }

  companion object {
    const val ERR_HOST_INACTIVE = "ERR_HOST_INACTIVE"
  }
}
