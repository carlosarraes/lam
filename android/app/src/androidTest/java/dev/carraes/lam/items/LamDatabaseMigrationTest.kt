package dev.carraes.lam.items

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import java.time.Instant
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class LamDatabaseMigrationTest {
    @Test fun shippedV4DropsSavedContentAndPreservesPendingItems() = runTest {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val name = "v4-migration-${java.util.UUID.randomUUID()}.db"
        val schema = InstrumentationRegistry.getInstrumentation().context.assets
            .open("dev.carraes.lam.items.LamDatabase/4.json").bufferedReader().use { JSONObject(it.readText()).getJSONObject("database") }
        try {
            context.openOrCreateDatabase(name, Context.MODE_PRIVATE, null).use { old ->
                val entities = schema.getJSONArray("entities")
                for (index in 0 until entities.length()) {
                    val entity = entities.getJSONObject(index)
                    val table = entity.getString("tableName")
                    old.execSQL(entity.getString("createSql").replace("\${TABLE_NAME}", table))
                    entity.optJSONArray("indices")?.let { indices ->
                        for (i in 0 until indices.length()) old.execSQL(indices.getJSONObject(i).getString("createSql").replace("\${TABLE_NAME}", table))
                    }
                }
                val setup = schema.getJSONArray("setupQueries")
                for (i in 0 until setup.length()) old.execSQL(setup.getString(i))
                old.execSQL("INSERT INTO articles VALUES ('account','saved','{}','Private content',NULL)")
                old.execSQL("""INSERT INTO items (createdAtEpoch,id,title,body,sourceHost,sourceProject,priority,choices,checks,link,status,createdAt,version,kind,pendingAt)
                    VALUES (0,'pending','Tracked work','Preserve this','host','lam','NORMAL','[]','[]','','OPEN','2026-09-04T00:00:00Z',7,'FYI','2026-09-04T12:00:00Z')""")
                old.version = 4
            }
            val database = Room.databaseBuilder(context, LamDatabase::class.java, name).addMigrations(LamDatabase.MIGRATION_4_5).build()
            try {
                val item = RoomItemStorage(database).get("pending")!!.canonical
                assertEquals("Preserve this", item.body)
                assertEquals("2026-09-04T12:00:00Z", item.pendingAt)
                assertEquals(7L, item.version)
                database.openHelper.writableDatabase.query("SELECT name FROM sqlite_master WHERE type='table' AND name='articles'").use {
                    assertEquals(0, it.count)
                }
            } finally { database.close() }
        } finally { context.deleteDatabase(name) }
    }

    @Test fun shippedV1UpgradesWithoutLosingLegacyChecklistHistoryOrSyncData() = runTest {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val name = "fyi-migration-${java.util.UUID.randomUUID()}.db"
        val schema = InstrumentationRegistry.getInstrumentation().context.assets
            .open("dev.carraes.lam.items.LamDatabase/1.json").bufferedReader().use { JSONObject(it.readText()).getJSONObject("database") }
        try {
            context.openOrCreateDatabase(name, Context.MODE_PRIVATE, null).use { old ->
                val entities = schema.getJSONArray("entities")
                for (index in 0 until entities.length()) {
                    val entity = entities.getJSONObject(index)
                    val table = entity.getString("tableName")
                    old.execSQL(entity.getString("createSql").replace("\${TABLE_NAME}", table))
                    entity.optJSONArray("indices")?.let { indices ->
                        for (i in 0 until indices.length()) old.execSQL(indices.getJSONObject(i).getString("createSql").replace("\${TABLE_NAME}", table))
                    }
                }
                val setup = schema.getJSONArray("setupQueries")
                for (i in 0 until setup.length()) old.execSQL(setup.getString(i))
                old.execSQL("""INSERT INTO items (createdAtEpoch,id,title,body,sourceHost,sourceProject,priority,choices,checks,link,status,createdAt,version)
                    VALUES (0,'legacy','Old checklist','Preserve this','host','lam','LOW','[]','[{"label":"Review","done":false,"at":null}]','','OPEN','2026-09-04T00:00:00Z',4294967296),
                    (0,'closed','Closed request','Old history','host','lam','NORMAL','[]','[]','','DISMISSED','2026-09-04T00:00:00Z',1)""")
                old.execSQL("INSERT INTO sync_metadata VALUES (1, '2026-09-04T12:00:00Z')")
                old.execSQL("INSERT INTO history_membership VALUES ('saved-query', 'closed')")
                old.version = 1
            }
            val database = Room.databaseBuilder(context, LamDatabase::class.java, name).addMigrations(LamDatabase.MIGRATION_1_2, LamDatabase.MIGRATION_2_3, LamDatabase.MIGRATION_3_4, LamDatabase.MIGRATION_4_5).build()
            try {
                val store = RoomItemStorage(database)
                val legacy = store.get("legacy")!!.canonical
                assertEquals(ItemKindDto.REQUEST, legacy.kind)
                assertNull(legacy.seenAt)
                assertNull(legacy.pendingAt)
                assertEquals(PriorityDto.LOW, legacy.priority)
                assertEquals("Preserve this", legacy.body)
                assertEquals(listOf(CheckDto("Review", false, null)), legacy.checks)
                assertEquals(4294967296L, legacy.version)
                assertEquals(Instant.parse("2026-09-04T12:00:00Z"), store.lastSuccess())
                assertEquals("closed", store.history("saved-query").first().single().canonical.id)
                assertEquals("legacy", store.openItems().first().single().canonical.id)
                val pending = legacy.copy(id = "pending", pendingAt = "2026-09-04T12:00:00Z")
                store.upsert(listOf(ItemMapper.toEntity(pending)))
                assertEquals(pending, store.get("pending")!!.canonical)
                val seen = legacy.copy(id = "notice", kind = ItemKindDto.FYI, checks = emptyList(), status = StatusDto.DISMISSED,
                    seenAt = "2026-09-04T12:00:00Z", resolvedAt = "2026-09-04T12:00:00Z")
                store.upsert(listOf(ItemMapper.toEntity(seen)))
                assertEquals(seen, store.get("notice")!!.canonical)
            } finally { database.close() }
        } finally { context.deleteDatabase(name) }
    }
}
