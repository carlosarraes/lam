package dev.carraes.lam.items

import androidx.room.Database
import androidx.room.Entity
import androidx.room.Index
import androidx.room.RoomDatabase
import androidx.room.TypeConverters

@Entity(tableName = "history_membership", primaryKeys = ["queryKey", "itemId"], indices = [Index("itemId")])
data class HistoryMembership(val queryKey: String, val itemId: String)

@Database(entities = [ItemEntity::class, SyncMetadata::class, HistoryMembership::class], version = 1, exportSchema = true)
@TypeConverters(ItemConverters::class)
abstract class LamDatabase : RoomDatabase() {
    abstract fun itemDao(): ItemDao
    abstract fun syncMetadataDao(): SyncMetadataDao
}
