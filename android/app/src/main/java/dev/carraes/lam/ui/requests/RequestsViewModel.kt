package dev.carraes.lam.ui.requests

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import dev.carraes.lam.items.*
import java.time.Clock
import java.time.Instant
import java.time.Duration
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.launch

data class RequestsState(
    val items: List<Item> = emptyList(),
    val query: String = "",
    val type: ItemTypeDto? = null,
    val priority: PriorityDto? = null,
    val loading: Boolean = true,
    val refreshing: Boolean = false,
    val stale: Boolean = false,
    val lastSuccess: Instant? = null,
    val actionsEnabled: Boolean = false,
    val now: Instant = Instant.now(),
) {
    val actionableCount: Int get() = items.count { !it.isFyi }
    val informationalCount: Int get() = items.count { it.isFyi }
}

class RequestsViewModel(repository: ItemRepository, private val reconcile: suspend () -> Boolean, clock: Clock = Clock.systemUTC()) : ViewModel() {
    private data class Filters(val query: String = "", val type: ItemTypeDto? = null, val priority: PriorityDto? = null)
    private val filters = MutableStateFlow(Filters())
    // This updates labels only while a screen collects state. It never performs network work.
    private val time = flow {
        while (true) {
            emit(clock.instant())
            delay(60_000)
        }
    }
    val state: StateFlow<RequestsState> = combine(repository.openItems, repository.syncState, filters, time) { items, sync, filter, now ->
        val query = filter.query.trim()
        val visible = items.filter { item ->
            item.status == StatusDto.OPEN &&
                (filter.type == null || item.requestType == filter.type) &&
                (filter.priority == null || item.priority == filter.priority) &&
                (query.isEmpty() || listOf(item.title, item.agentDisplay, item.body).any { it.contains(query, ignoreCase = true) })
        }.sortedWith(compareByDescending<Item> { it.priority.ordinal }.thenByDescending { it.createdAt }.thenBy { it.id })
        RequestsState(visible, filter.query, filter.type, filter.priority,
            loading = items.isEmpty() && (sync == SyncState.Idle || sync == SyncState.Refreshing),
            refreshing = sync == SyncState.Refreshing,
            stale = sync is SyncState.Stale,
            lastSuccess = when (sync) { is SyncState.Current -> sync.lastSuccess; is SyncState.Stale -> sync.lastSuccess; else -> null },
            actionsEnabled = sync.mutationsEnabled,
            now = now)
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(), RequestsState(now = clock.instant()))

    fun setQuery(query: String) { filters.update { it.copy(query = query) } }
    fun setType(type: ItemTypeDto?) { filters.update { it.copy(type = type) } }
    fun setPriority(priority: PriorityDto?) { filters.update { it.copy(priority = priority) } }
    fun clearFilters() { filters.update { it.copy(type = null, priority = null) } }
    fun refresh() { viewModelScope.launch { reconcile() } }
}

val Item.requestType: ItemTypeDto get() = when {
    checks.isNotEmpty() -> ItemTypeDto.CHECKLIST
    choices.isNotEmpty() -> ItemTypeDto.CHOICE
    else -> ItemTypeDto.PLAIN
}
val Item.missingRecommendation: Boolean get() = !isFyi && checks.isEmpty() && recommendation.isNullOrBlank()
fun requestAge(item: Item, now: Instant): String {
    val created = runCatching { Instant.parse(item.createdAt) }.getOrNull() ?: return "Unknown age"
    val minutes = Duration.between(created, now).toMinutes().coerceAtLeast(0)
    return when {
        minutes < 1 -> "Just now"
        minutes < 60 -> "${minutes}m"
        minutes < 1440 -> "${minutes / 60}h"
        else -> "${minutes / 1440}d"
    }
}
