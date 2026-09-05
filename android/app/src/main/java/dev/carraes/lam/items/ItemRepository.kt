package dev.carraes.lam.items

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

@Serializable
data class HistoryQuery(val query: String? = null, val priority: PriorityDto? = null, val type: ItemTypeDto? = null) {
    internal val key: String get() = Json.encodeToString(this)
}

data class HistoryResult(val succeeded: Boolean, val nextCursor: String?)

sealed interface FinalAnswer {
    data class Choice(val value: String) : FinalAnswer
    data class Text(val value: String) : FinalAnswer
    data object Dismiss : FinalAnswer
}

interface ItemRepository {
    val openItems: Flow<List<Item>>
    val syncState: StateFlow<SyncState>
    val errors: Flow<Exception>
    fun item(id: String): Flow<Item?>
    fun history(query: HistoryQuery = HistoryQuery()): Flow<List<Item>>
    suspend fun refresh(): Boolean
    suspend fun refreshItem(id: String): Boolean
    suspend fun refreshHistory(query: HistoryQuery = HistoryQuery(), cursor: String? = null): HistoryResult
    suspend fun answer(id: String, answer: FinalAnswer): Boolean
    suspend fun setCheck(id: String, index: Int, done: Boolean): Boolean
    suspend fun unpair()
}
