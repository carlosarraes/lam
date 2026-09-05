package dev.carraes.lam.items

import androidx.room.Room
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.time.Instant
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class ItemDaoTest {
    private lateinit var database: LamDatabase
    private lateinit var store: RoomItemStorage

    @Before fun setUp() {
        database = Room.inMemoryDatabaseBuilder(ApplicationProvider.getApplicationContext(), LamDatabase::class.java).build()
        store = RoomItemStorage(database)
    }

    @After fun tearDown() { database.close() }

    @Test fun jsonArraysAndEveryCanonicalFieldRoundTrip() = runTest {
        val original = row().copy(canonical = row().canonical.copy(
            name = "Agent", choices = listOf("quote\"", "comma,", "unicode ☕"),
            checks = listOf(CheckDto("line\nbreak", true, "2026-09-04T10:00:00Z")),
            responseChoice = "quote\"", responseText = "response", responseBy = ResponseByDto.CLI,
            recommendation = "why", recommendedChoice = "quote\"", version = 42,
        ))
        store.upsert(listOf(original))
        assertEquals(original, store.get("a"))
    }

    @Test fun openQueueSortsPriorityThenNewestAndPreservesClosedRows() = runTest {
        store.upsert(listOf(row("missing"), row("closed", StatusDto.RESOLVED)))
        store.reconcile(listOf(
            row("low", priority = PriorityDto.LOW), row("normal-new", created = "2026-09-04T11:00:00Z"),
            row("critical", priority = PriorityDto.CRITICAL), row("normal-old"),
        ), NOW)
        assertEquals(listOf("critical", "normal-new", "normal-old", "low"), store.openItems().first().map { it.canonical.id })
        assertNull(store.get("missing"))
        assertNotNull(store.get("closed"))
        assertEquals(NOW, store.lastSuccess())
        store.reconcile(emptyList(), NOW.plusSeconds(1))
        assertTrue(store.openItems().first().isEmpty())
        assertNotNull(store.get("closed"))
    }

    @Test fun failedTransactionRollsBackUpsertsDeletionAndSuccessTogether() = runTest {
        store.reconcile(listOf(row("original")), NOW)
        database.openHelper.writableDatabase.execSQL(
            "CREATE TRIGGER fail_sync_metadata BEFORE INSERT ON sync_metadata BEGIN SELECT RAISE(ABORT, 'forced sync failure'); END",
        )
        var failed = false
        try {
            store.reconcile(listOf(row("replacement")), NOW.plusSeconds(1))
        } catch (_: android.database.sqlite.SQLiteException) {
            failed = true
        }
        assertTrue("metadata failure must escape reconciliation", failed)
        assertEquals(listOf("original"), store.openItems().first().map { it.canonical.id })
        assertNull(store.get("replacement"))
        assertEquals(NOW, store.lastSuccess())
    }

    @Test fun olderCanonicalResponseAndLateRollbackCannotOverwriteNewerVersion() = runTest {
        val snapshot = row()
        store.upsert(listOf(snapshot))
        val optimistic = snapshot.copy(canonical = snapshot.canonical.copy(checks = listOf(CheckDto("step", true, null))), optimisticTag = "write-1")
        store.optimistic(snapshot, optimistic)
        assertTrue(store.get("a")!!.canonical.checks.single().done)
        val canonical = row(status = StatusDto.RESOLVED).let { it.copy(canonical = it.canonical.copy(version = 3)) }
        store.upsert(listOf(canonical))
        store.rollback(snapshot, "write-1")
        store.upsert(listOf(snapshot))
        assertEquals(canonical, store.get("a"))
    }

    @Test fun historyMembershipIsSeparateAndSortedByEffectiveClosureTime() = runTest {
        store.reconcile(listOf(row()), NOW)
        store.cacheHistory("first", listOf(row("old", StatusDto.RESOLVED), row("expired", StatusDto.EXPIRED)), true)
        store.cacheHistory("first", listOf(row("old", StatusDto.RESOLVED)), false)
        assertEquals(listOf("expired", "old"), store.history("first").first().map { it.canonical.id })
        store.cacheHistory("second", emptyList(), true)
        assertEquals(2, store.history("first").first().size)
        store.cacheHistory("first", emptyList(), true)
        assertTrue(store.history("first").first().isEmpty())
        assertNotNull(store.get("old"))
        assertEquals(listOf("a"), store.openItems().first().map { it.canonical.id })
        store.clear()
        assertNull(store.get("old"))
        assertNull(store.lastSuccess())
        assertTrue(store.openItems().first().isEmpty())
    }

    @Test fun derivedExpirationCannotBeReopenedByAnOlderSnapshotWithTheSameVersion() = runTest {
        store.upsert(listOf(row(status = StatusDto.EXPIRED)))
        store.upsert(listOf(row()))
        assertEquals(StatusDto.EXPIRED, store.get("a")!!.canonical.status)
        assertTrue(store.openItems().first().isEmpty())
    }

    private fun row(id: String = "a", status: StatusDto = StatusDto.OPEN, priority: PriorityDto = PriorityDto.NORMAL, created: String = "2026-09-04T09:00:00Z") = ItemMapper.toEntity(ItemDto(
        id, null, "Title", "Body", "host", "project", priority, emptyList(), listOf(CheckDto("step", false, null)),
        null, null, "https://example.com", status, null, null, null, created,
        if (status == StatusDto.RESOLVED) "2026-09-04T10:00:00Z" else null,
        if (status == StatusDto.EXPIRED) "2026-09-04T11:00:00Z" else null, 1,
    ))

    private companion object { val NOW: Instant = Instant.parse("2026-09-04T12:00:00Z") }
}
