package dev.carraes.lam.articles

import android.content.Context
import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import dev.carraes.lam.items.LamDatabase
import dev.carraes.lam.items.RoomItemStorage
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.flow.first
import java.time.Instant
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ArticleStorageTest {
    @Test fun v2MigrationPreservesFyiAndAccountCleanupErasesArticles() = runTest {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val name = "articles-migration-${java.util.UUID.randomUUID()}.db"
        try {
            val schema = InstrumentationRegistry.getInstrumentation().context.assets
                .open("dev.carraes.lam.items.LamDatabase/2.json").bufferedReader().use { JSONObject(it.readText()).getJSONObject("database") }
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
                old.execSQL("""INSERT INTO items (createdAtEpoch,id,title,body,sourceHost,sourceProject,priority,choices,checks,link,status,createdAt,version,kind,seenAt)
                    VALUES (0,'fyi','Notice','Keep body','host','lam','LOW','[]','[]','','DISMISSED','2026-09-04T00:00:00Z',4294967296,'FYI','2026-09-05T00:00:00Z')""")
                old.execSQL("INSERT INTO sync_metadata VALUES (1, '2026-09-04T12:00:00Z')")
                old.execSQL("INSERT INTO history_membership VALUES ('saved-query', 'fyi')")
                old.version = 2
            }
            val database = Room.databaseBuilder(context, LamDatabase::class.java, name).addMigrations(LamDatabase.MIGRATION_2_3).build()
            try {
                val items = RoomItemStorage(database)
                assertEquals("Keep body", items.get("fyi")!!.canonical.body)
                assertEquals("2026-09-05T00:00:00Z", items.get("fyi")!!.canonical.seenAt)
                assertEquals(4294967296L, items.get("fyi")!!.canonical.version)
                assertEquals(Instant.parse("2026-09-04T12:00:00Z"), items.lastSuccess())
                assertEquals("fyi", items.history("saved-query").first().single().canonical.id)
                val articles = RoomArticleStorage(database)
                val row = ArticleEntity("account-a", "id", "{}", "private text", "digest")
                assertTrue(articles.save(listOf(row)) { true })
                assertNull(articles.get("account-b", "id"))
                assertFalse(articles.save(listOf(row.copy(id = "obsolete"))) { false })
                assertNull(articles.get("account-a", "obsolete"))
                items.clear()
                assertTrue(articles.list("account-a").isEmpty())
            } finally { database.close() }
        } finally { context.deleteDatabase(name) }
    }
}
