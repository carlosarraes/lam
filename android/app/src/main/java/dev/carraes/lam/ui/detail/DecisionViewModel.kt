package dev.carraes.lam.ui.detail

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.carraes.lam.items.*
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.launch

data class AnswerConfirmation(val answer: FinalAnswer, val version: Long, val token: Long)

data class DecisionState(
    val item: Item? = null,
    val loading: Boolean = true,
    val refreshing: Boolean = false,
    val loadFailed: Boolean = false,
    val sync: SyncState = SyncState.Idle,
    val submitting: Boolean = false,
    val answerFailed: Boolean = false,
    val replyOpen: Boolean = false,
    val quickOpen: Boolean = false,
    val reply: String = "",
    val confirmation: AnswerConfirmation? = null,
) {
    val missing: Boolean get() = item == null && !loading && !loadFailed
    val actionsEnabled: Boolean get() = item?.status == StatusDto.OPEN && sync.mutationsEnabled && !submitting && !loadFailed
    val replyBytes: Int get() = reply.toByteArray(Charsets.UTF_8).size
    val replyValid: Boolean get() = reply.isNotBlank() && replyBytes <= 8 * 1024
}

class DecisionViewModel(
    private val id: String,
    private val repository: ItemRepository,
    private val reconcile: suspend () -> Boolean = repository::refresh,
) : ViewModel() {
    private val mutableState = MutableStateFlow(DecisionState())
    val state: StateFlow<DecisionState> = mutableState.asStateFlow()
    private var confirmationToken = 0L

    init {
        viewModelScope.launch {
            combine(repository.item(id), repository.syncState) { item, sync -> item to sync }.collect { (item, sync) ->
                mutableState.update { old ->
                    val closed = item?.status != StatusDto.OPEN
                    old.copy(item = item, sync = sync,
                        confirmation = old.confirmation?.takeIf { !closed && sync.mutationsEnabled && it.version == item.version },
                        replyOpen = old.replyOpen && !closed,
                        quickOpen = old.quickOpen && !closed && sync.mutationsEnabled,
                        reply = if (closed) "" else old.reply)
                }
            }
        }
        refresh()
    }

    fun refresh() {
        if (state.value.refreshing || state.value.submitting) return
        mutableState.update { it.copy(refreshing = true, loading = it.item == null, confirmation = null) }
        viewModelScope.launch {
            var succeeded = false
            try {
                if (!repository.syncState.value.mutationsEnabled) reconcile()
                succeeded = repository.refreshItem(id)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                // Repositories normally report failure as false; keep cached content if a local read fails.
            } finally {
                val missing = (repository.syncState.value as? SyncState.Stale)?.error.let { it is ApiError && it.statusCode == 404 }
                mutableState.update { it.copy(loading = false, refreshing = false, loadFailed = !succeeded && !missing) }
            }
        }
    }

    fun choose(choice: String) {
        val item = state.value.item ?: return
        if (!canAnswer() || choice !in item.choices) return
        requestConfirmation(FinalAnswer.Choice(choice), item.version)
    }

    fun complete() {
        val item = state.value.item ?: return
        if (canAnswer() && item.choices.isEmpty() && item.checks.isEmpty()) {
            requestConfirmation(FinalAnswer.Complete, item.version)
        }
    }

    fun quickResponse() {
        if (!canAnswer()) return
        val plain = state.value.item!!.let { it.checks.isEmpty() && it.choices.isEmpty() }
        mutableState.update { it.copy(quickOpen = true, replyOpen = plain, answerFailed = false) }
    }

    fun closeQuickResponse() {
        mutableState.update { it.copy(quickOpen = false, replyOpen = false, confirmation = null) }
    }

    fun dismissRequest() {
        if (canAnswer()) requestConfirmation(FinalAnswer.Dismiss, state.value.item!!.version)
    }

    fun writeReply() {
        if (canAnswer()) mutableState.update { it.copy(replyOpen = true, answerFailed = false) }
    }

    fun editReply(text: String) {
        if (state.value.replyOpen && !state.value.submitting) mutableState.update { it.copy(reply = text, confirmation = null) }
    }

    fun reviewReply() {
        val current = state.value
        if (!canAnswer() || !current.replyOpen || !current.replyValid) return
        requestConfirmation(FinalAnswer.Text(current.reply), current.item!!.version)
    }

    fun dismissReply() {
        mutableState.update {
            val plain = it.item?.let { item -> item.checks.isEmpty() && item.choices.isEmpty() } == true
            it.copy(replyOpen = false, confirmation = null, quickOpen = it.quickOpen && !plain)
        }
    }
    fun dismissConfirmation() { mutableState.update { it.copy(confirmation = null) } }

    fun confirm(confirmation: AnswerConfirmation) {
        val current = state.value
        if (!canAnswer() || current.confirmation != confirmation || current.item?.version != confirmation.version) return
        // Consume synchronously, before launching, so two taps cannot start two repository operations.
        mutableState.update { it.copy(confirmation = null, replyOpen = false, quickOpen = false, submitting = true, answerFailed = false) }
        viewModelScope.launch {
            var succeeded = false
            try {
                succeeded = repository.answer(id, confirmation.answer)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Exception) {
                // Do not retry final answers. The repository owns canonical reconciliation.
            } finally {
                mutableState.update { it.copy(submitting = false, answerFailed = !succeeded) }
            }
        }
    }

    private fun canAnswer() = state.value.actionsEnabled && repository.syncState.value.mutationsEnabled

    private fun requestConfirmation(answer: FinalAnswer, version: Long) {
        mutableState.update { it.copy(confirmation = AnswerConfirmation(answer, version, ++confirmationToken), answerFailed = false) }
    }
}
