package dev.carraes.lam.items

import dev.carraes.lam.diagnostics.DiagnosticData
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow

enum class UnpairResult { REVOKED, UNAVAILABLE, SESSION_CHANGED }

interface DeviceSettings {
    val device: Flow<PairedServer?>
    val reconciliationSession: StateFlow<Long?>
    val syncState: StateFlow<SyncState>
    suspend fun revoke(expectedSession: Long): UnpairResult
    suspend fun eraseLocal(expectedSession: Long): Boolean
    suspend fun diagnosticData(): DiagnosticData
}
