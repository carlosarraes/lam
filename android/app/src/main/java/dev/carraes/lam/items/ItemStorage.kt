package dev.carraes.lam.items

import java.time.Instant
import kotlinx.coroutines.flow.Flow

internal interface ItemStorage {
    fun openItems(): Flow<List<ItemEntity>>
    fun item(id: String): Flow<ItemEntity?>
    fun history(key: String): Flow<List<ItemEntity>>
    suspend fun get(id: String): ItemEntity?
    suspend fun lastSuccess(): Instant?
    suspend fun counts(): dev.carraes.lam.diagnostics.ItemCounts
    suspend fun upsert(items: List<ItemEntity>)
    suspend fun reconcile(items: List<ItemEntity>, at: Instant)
    suspend fun cacheHistory(key: String, items: List<ItemEntity>, replace: Boolean)
    suspend fun optimistic(snapshot: ItemEntity, changed: ItemEntity)
    suspend fun rollback(snapshot: ItemEntity, tag: String)
    suspend fun clear()
}
