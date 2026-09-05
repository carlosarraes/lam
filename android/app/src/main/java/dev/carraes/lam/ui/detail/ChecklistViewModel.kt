package dev.carraes.lam.ui.detail

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.carraes.lam.items.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.launch

data class ChecklistState(
    val item: Item? = null,
    val sync: SyncState = SyncState.Idle,
    val saving: Boolean = false,
    val failure: Long? = null,
) {
    val actionsEnabled: Boolean get() = item?.status == StatusDto.OPEN && sync.mutationsEnabled && !saving
}

class ChecklistViewModel(private val id: String, private val repository: ItemRepository) : ViewModel() {
    private val mutableState = MutableStateFlow(ChecklistState())
    val state = mutableState.asStateFlow()
    private var failureToken = 0L

    init {
        viewModelScope.launch {
            combine(repository.item(id), repository.syncState) { item, sync -> item to sync }.collect { (item, sync) ->
                mutableState.update { it.copy(item = item, sync = sync) }
            }
        }
    }

    fun setCheck(index: Int, done: Boolean) {
        val current = state.value
        val check = current.item?.checks?.getOrNull(index) ?: return
        if (!current.actionsEnabled || !repository.syncState.value.mutationsEnabled || check.done == done) return
        mutableState.update { it.copy(saving = true, failure = null) }
        viewModelScope.launch {
            var succeeded = false
            try {
                succeeded = repository.setCheck(id, index, done)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                // The repository owns the optimistic update and rollback, including ambiguous recovery.
            } finally {
                mutableState.update { it.copy(saving = false, failure = if (succeeded) null else ++failureToken) }
            }
        }
    }

    fun consumeFailure(token: Long) {
        mutableState.update { if (it.failure == token) it.copy(failure = null) else it }
    }
}
