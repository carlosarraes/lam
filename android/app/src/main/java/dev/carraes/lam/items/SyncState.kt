package dev.carraes.lam.items

import java.time.Instant

sealed interface SyncState {
    val mutationsEnabled: Boolean get() = this is Current
    data object Idle : SyncState
    data object Refreshing : SyncState
    data class Current(val lastSuccess: Instant) : SyncState
    data class Stale(val lastSuccess: Instant?, val error: Exception) : SyncState
    data object Revoked : SyncState
}
