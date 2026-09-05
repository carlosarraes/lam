package dev.carraes.lam.ui.history

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.carraes.lam.items.*
import java.time.Instant
import kotlinx.coroutines.Job
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.*

data class HistoryState(
    val items: List<Item> = emptyList(),
    val query: HistoryQuery = HistoryQuery(),
    val loading: Boolean = false,
    val loadFailed: Boolean = false,
    val nextCursor: String? = null,
    val stale: Boolean = false,
    val lastSuccess: Instant? = null,
)

class HistoryViewModel(private val repository: ItemRepository) : ViewModel() {
    private val mutableState = MutableStateFlow(HistoryState())
    val state = mutableState.asStateFlow()
    private var revision = 0L
    private var cache: Job? = null
    private var page: Job? = null

    init {
        viewModelScope.launch {
            repository.syncState.collect { sync ->
                mutableState.update { it.copy(stale = sync is SyncState.Stale, lastSuccess = when (sync) {
                    is SyncState.Current -> sync.lastSuccess
                    is SyncState.Stale -> sync.lastSuccess
                    else -> it.lastSuccess
                }) }
            }
        }
        select(HistoryQuery())
    }

    fun setQuery(query: String) = change(state.value.query.copy(query = query.takeIf { it.isNotBlank() }))
    fun setPriority(priority: PriorityDto?) = change(state.value.query.copy(priority = priority))
    fun setType(type: ItemTypeDto?) = change(state.value.query.copy(type = type))
    fun clearFilters() = change(state.value.query.copy(priority = null, type = null))
    fun refresh() { if (!state.value.loading) fetch(null) }
    fun loadMore() { state.value.nextCursor?.let { if (!state.value.loading) fetch(it) } }

    private fun change(query: HistoryQuery) { if (query != state.value.query) select(query) }

    private fun select(query: HistoryQuery) {
        revision++
        cache?.cancel()
        page?.cancel()
        mutableState.update { it.copy(query = query, items = emptyList(), nextCursor = null, loading = false, loadFailed = false) }
        val selected = revision
        cache = viewModelScope.launch {
            repository.history(query).collect { items ->
                if (selected == revision) mutableState.update { it.copy(items = items.filter { row -> row.status != StatusDto.OPEN }
                    .sortedWith(compareByDescending<Item> { row -> row.effectiveClosureTime?.let(Instant::parse) }.thenByDescending { row -> row.id })) }
            }
        }
        fetch(null)
    }

    private fun fetch(cursor: String?) {
        val selected = revision
        val query = state.value.query
        mutableState.update { it.copy(loading = true, loadFailed = false) }
        page = viewModelScope.launch {
            val result = repository.refreshHistory(query, cursor)
            if (selected == revision) mutableState.update {
                it.copy(loading = false, loadFailed = !result.succeeded,
                    nextCursor = if (result.succeeded) result.nextCursor else it.nextCursor)
            }
        }
    }
}

val Item.effectiveClosureTime: String? get() = if (status == StatusDto.EXPIRED) expiresAt else resolvedAt
