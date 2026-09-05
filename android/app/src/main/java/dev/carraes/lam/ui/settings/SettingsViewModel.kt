package dev.carraes.lam.ui.settings

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.carraes.lam.items.*
import dev.carraes.lam.diagnostics.Diagnostics
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.*

enum class UnpairConfirmation { REMOTE, LOCAL_ERASE }
data class SettingsState(
    val device: PairedServer? = null,
    val sync: SyncState = SyncState.Idle,
    val confirmation: UnpairConfirmation? = null,
    val busy: Boolean = false,
    val failed: Boolean = false,
    val diagnostics: String? = null,
    val lastSuccess: java.time.Instant? = null,
)
class SettingsViewModel(private val repository: DeviceSettings, private val diagnostics: Diagnostics) : ViewModel() {
    private val mutableState = MutableStateFlow(SettingsState())
    val state = mutableState.asStateFlow()
    private var confirmedSession: Long? = null

    init {
        viewModelScope.launch { repository.device.collect { device -> mutableState.update { it.copy(device = device) } } }
        viewModelScope.launch { repository.syncState.collect { sync -> mutableState.update { it.copy(sync = sync,
            lastSuccess = when (sync) { is SyncState.Current -> sync.lastSuccess; is SyncState.Stale -> sync.lastSuccess; else -> it.lastSuccess }) } } }
        viewModelScope.launch {
            try { val data = repository.diagnosticData(); mutableState.update { it.copy(lastSuccess = data.lastSuccess) } }
            catch (cancelled: CancellationException) { throw cancelled }
            catch (_: Exception) { mutableState.update { it.copy(failed = true) } }
        }
        viewModelScope.launch { repository.reconciliationSession.collect { session ->
            if (confirmedSession != session) {
                confirmedSession = null
                mutableState.update { it.copy(confirmation = null, busy = false, diagnostics = null) }
            }
        } }
    }

    fun requestUnpair() {
        if (state.value.busy) return
        confirmedSession = repository.reconciliationSession.value ?: return
        mutableState.update { it.copy(confirmation = UnpairConfirmation.REMOTE, failed = false) }
    }

    fun cancelUnpair() {
        if (state.value.busy) return
        confirmedSession = null
        mutableState.update { it.copy(confirmation = null) }
    }

    fun confirmUnpair() {
        if (state.value.busy) return
        val session = confirmedSession ?: return
        val confirmation = state.value.confirmation ?: return
        if (repository.reconciliationSession.value != session) { cancelUnpair(); return }
        mutableState.update { it.copy(busy = true, failed = false) }
        viewModelScope.launch {
            try {
                val result = if (confirmation == UnpairConfirmation.REMOTE) repository.revoke(session)
                    else if (repository.eraseLocal(session)) UnpairResult.REVOKED else UnpairResult.SESSION_CHANGED
                if (confirmedSession == session) mutableState.update { it.copy(busy = false,
                    confirmation = if (result == UnpairResult.UNAVAILABLE) UnpairConfirmation.LOCAL_ERASE else null) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                if (confirmedSession == session) mutableState.update { it.copy(busy = false, confirmation = null, failed = true) }
            }
        }
    }

    fun copyDiagnostics() {
        viewModelScope.launch {
            try {
                val text = diagnostics.render(repository.diagnosticData())
                mutableState.update { it.copy(diagnostics = text, failed = false) }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) { mutableState.update { it.copy(failed = true) } }
        }
    }

    fun diagnosticsCopied() { mutableState.update { it.copy(diagnostics = null) } }
}
