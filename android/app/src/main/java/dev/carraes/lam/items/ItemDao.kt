package dev.carraes.lam.items

import androidx.room.Dao
import androidx.room.Insert
import androidx.room.OnConflictStrategy
import androidx.room.Query
import androidx.room.Transaction
import androidx.room.Upsert
import kotlinx.coroutines.flow.Flow

@Dao
abstract class ItemDao {
    @Query("SELECT COUNT(*) AS total, COALESCE(SUM(CASE WHEN status = 'OPEN' THEN 1 ELSE 0 END), 0) AS open FROM items")
    abstract suspend fun counts(): dev.carraes.lam.diagnostics.ItemCounts

    @Query("SELECT * FROM items WHERE status = 'OPEN' ORDER BY CASE priority WHEN 'CRITICAL' THEN 0 WHEN 'NORMAL' THEN 1 ELSE 2 END, createdAtEpoch DESC, id DESC")
    abstract fun observeOpen(): Flow<List<ItemEntity>>

    @Query("SELECT * FROM items WHERE status != 'OPEN' ORDER BY effectiveClosureAt DESC, id DESC")
    abstract fun observeCachedHistory(): Flow<List<ItemEntity>>

    @Query("SELECT * FROM items WHERE id = :id")
    abstract fun observeItem(id: String): Flow<ItemEntity?>

    @Query("SELECT * FROM items WHERE id = :id")
    abstract suspend fun get(id: String): ItemEntity?

    @Query("SELECT items.* FROM items INNER JOIN history_membership ON items.id = history_membership.itemId WHERE history_membership.queryKey = :key AND items.status != 'OPEN' ORDER BY effectiveClosureAt DESC, id DESC")
    abstract fun observeHistory(key: String): Flow<List<ItemEntity>>

    @Upsert
    protected abstract suspend fun write(item: ItemEntity)

    @Transaction
    open suspend fun upsertAll(items: List<ItemEntity>) {
        items.forEach { incoming ->
            val current = get(incoming.canonical.id)
            // Expiration is derived without a version increment. Equal-version OPEN cannot undo it.
            val reopensClosed = current != null &&
                current.canonical.status != StatusDto.OPEN &&
                incoming.canonical.status == StatusDto.OPEN &&
                incoming.canonical.version == current.canonical.version
            if ((current == null || incoming.canonical.version >= current.canonical.version) && !reopensClosed) {
                write(incoming)
            }
        }
    }

    @Query("DELETE FROM items WHERE status = 'OPEN' AND id NOT IN (:ids)")
    abstract suspend fun deleteOpenNotIn(ids: List<String>)

    @Insert(onConflict = OnConflictStrategy.IGNORE)
    abstract suspend fun addHistory(membership: List<HistoryMembership>)

    @Query("DELETE FROM history_membership WHERE queryKey = :key")
    abstract suspend fun clearHistory(key: String)

    @Transaction
    open suspend fun optimistic(snapshot: ItemEntity, changed: ItemEntity) {
        if (get(snapshot.canonical.id) == snapshot) write(changed)
    }

    @Transaction
    open suspend fun rollback(snapshot: ItemEntity, tag: String) {
        val current = get(snapshot.canonical.id)
        if (current?.optimisticTag == tag && current.canonical.version == snapshot.canonical.version) write(snapshot)
    }

    @Query("DELETE FROM items")
    abstract suspend fun clearItems()

    @Query("DELETE FROM history_membership")
    abstract suspend fun clearHistory()
}
