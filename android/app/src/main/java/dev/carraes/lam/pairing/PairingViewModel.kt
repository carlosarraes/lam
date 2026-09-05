package dev.carraes.lam.pairing

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.combine
import dev.carraes.lam.items.SyncState
import kotlinx.coroutines.launch

sealed interface PairingState {
    data object Loading : PairingState
    data object Ready : PairingState
    data object Revoked : PairingState
    data object PermissionDenied : PairingState
    data object Scanning : PairingState
    data object Claiming : PairingState
    data class Failed(val problem: PairingProblem) : PairingState
    data class Paired(val stale: Boolean) : PairingState
}

class PairingViewModel(private val repository: PairingRepository, private val allowLocalHttp: Boolean,
    private val syncState: StateFlow<SyncState> = MutableStateFlow(SyncState.Idle),
) : ViewModel() {
    private val mutableState = MutableStateFlow<PairingState>(PairingState.Loading)
    val state = mutableState.asStateFlow()
    private var paired = false
    private var refreshing = false

    init {
        viewModelScope.launch {
            combine(repository.observePairing(), syncState) { server, sync -> server to sync }.collect { (server, sync) ->
                val changed = paired != (server != null)
                paired = server != null
                if (mutableState.value != PairingState.Claiming && (changed || mutableState.value == PairingState.Loading)) {
                    mutableState.value = if (paired) PairingState.Paired(true)
                        else if (sync == SyncState.Revoked) PairingState.Revoked else PairingState.Ready
                }
            }
        }
    }

    fun permissionResult(granted: Boolean) {
        if (paired || mutableState.value == PairingState.Claiming || mutableState.value == PairingState.Loading) return
        mutableState.value = if (granted) PairingState.Scanning else PairingState.PermissionDenied
    }

    fun cancelScan() {
        if (mutableState.value == PairingState.Scanning) mutableState.value = PairingState.Ready
    }

    fun cameraFailed() {
        if (mutableState.value == PairingState.Scanning) mutableState.value = PairingState.Failed(PairingProblem.CAMERA)
    }

    fun onQr(raw: String): Boolean {
        if (mutableState.value != PairingState.Scanning) return false
        val payload = try { PairingPayload.parse(raw, allowLocalHttp) } catch (error: PairingPayloadException) {
            mutableState.value = PairingState.Failed(error.problem)
            return false
        }
        mutableState.value = PairingState.Claiming
        viewModelScope.launch {
            val result = repository.pair(payload)
            paired = repository.observePairing().first() != null
            mutableState.value = when {
                result.problem != null -> PairingState.Failed(result.problem)
                paired -> PairingState.Paired(result.stale)
                syncState.value == SyncState.Revoked -> PairingState.Revoked
                else -> PairingState.Ready
            }
        }
        return true
    }

    fun retryRefresh() {
        if (!paired || refreshing) return
        refreshing = true
        viewModelScope.launch {
            try {
                val success = repository.refresh()
                if (paired) mutableState.value = PairingState.Paired(!success)
            } finally { refreshing = false }
        }
    }
}
