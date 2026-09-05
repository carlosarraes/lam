package dev.carraes.lam.items

import androidx.room.Dao
import androidx.room.Entity
import androidx.room.PrimaryKey
import androidx.room.Query
import androidx.room.Upsert

@Entity(tableName = "sync_metadata")
data class SyncMetadata(@PrimaryKey val id: Int = 1, val lastSuccess: String)

@Dao
interface SyncMetadataDao {
    @Query("SELECT lastSuccess FROM sync_metadata WHERE id = 1")
    suspend fun lastSuccess(): String?

    @Upsert
    suspend fun recordSuccess(metadata: SyncMetadata)

    @Query("DELETE FROM sync_metadata")
    suspend fun clear()
}
