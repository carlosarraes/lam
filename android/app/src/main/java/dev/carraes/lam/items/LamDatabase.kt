package dev.carraes.lam.items

import androidx.room.Database
import androidx.room.Entity
import androidx.room.Index
import androidx.room.RoomDatabase
import androidx.room.TypeConverters
import androidx.room.migration.Migration
import androidx.sqlite.db.SupportSQLiteDatabase

@Entity(tableName = "history_membership", primaryKeys = ["queryKey", "itemId"], indices = [Index("itemId")])
data class HistoryMembership(val queryKey: String, val itemId: String)

@Database(entities = [ItemEntity::class, SyncMetadata::class, HistoryMembership::class], version = 2, exportSchema = true)
@TypeConverters(ItemConverters::class)
abstract class LamDatabase : RoomDatabase() {
    companion object {
        val MIGRATION_1_2 = object : Migration(1, 2) {
            override fun migrate(db: SupportSQLiteDatabase) {
                db.execSQL("ALTER TABLE items ADD COLUMN kind TEXT NOT NULL DEFAULT 'REQUEST'")
                db.execSQL("ALTER TABLE items ADD COLUMN seenAt TEXT")
            }
        }
    }
    abstract fun itemDao(): ItemDao
    abstract fun syncMetadataDao(): SyncMetadataDao
}
