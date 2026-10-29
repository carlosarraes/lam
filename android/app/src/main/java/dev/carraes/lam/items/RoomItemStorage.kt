package dev.carraes.lam.items

import androidx.room.withTransaction
import java.time.Instant

internal class RoomItemStorage(private val database: LamDatabase) : ItemStorage {
    private val items = database.itemDao()
    private val metadata = database.syncMetadataDao()
    override fun openItems() = items.observeOpen()
    override fun cachedHistory() = items.observeCachedHistory()
    override fun item(id: String) = items.observeItem(id)
    override fun history(key: String) = items.observeHistory(key)
    override suspend fun get(id: String) = items.get(id)
    override suspend fun lastSuccess(): Instant? = metadata.lastSuccess()?.let(Instant::parse)
    override suspend fun counts() = items.counts()
    override suspend fun upsert(items: List<ItemEntity>) = this.items.upsertAll(items)
    override suspend fun reconcile(items: List<ItemEntity>, at: Instant) = database.withTransaction {
        this.items.upsertAll(items)
        this.items.deleteOpenNotIn(items.map { it.canonical.id })
        metadata.recordSuccess(SyncMetadata(lastSuccess = at.toString()))
    }
    override suspend fun cacheHistory(key: String, items: List<ItemEntity>, replace: Boolean) = database.withTransaction {
        this.items.upsertAll(items)
        if (replace) this.items.clearHistory(key)
        this.items.addHistory(items.map { HistoryMembership(key, it.canonical.id) })
    }
    override suspend fun optimistic(snapshot: ItemEntity, changed: ItemEntity) = items.optimistic(snapshot, changed)
    override suspend fun rollback(snapshot: ItemEntity, tag: String) = items.rollback(snapshot, tag)
    override suspend fun clear() = database.withTransaction {
        items.clearHistory()
        items.clearItems()
        metadata.clear()
    }
}
