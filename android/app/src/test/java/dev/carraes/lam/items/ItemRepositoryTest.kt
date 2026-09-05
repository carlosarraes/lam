package dev.carraes.lam.items

import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ItemRepositoryTest {
    @Test fun `mapping retains canonical fields and derives fallback source and closure`() {
        val dto = item().copy(name = " ", status = StatusDto.EXPIRED, expiresAt = "2026-09-04T10:00:00Z")
        val entity = ItemMapper.toEntity(dto)
        assertEquals(dto, entity.canonical)
        assertEquals("host:project", ItemMapper.toItem(entity).agentDisplay)
        assertEquals(Instant.parse("2026-09-04T10:00:00Z").toEpochMilli(), entity.effectiveClosureAt)
        assertEquals("Agent", ItemMapper.toItem(ItemMapper.toEntity(dto.copy(name = "Agent"))).agentDisplay)
    }

    @Test fun `only full refresh enables writes and removes missing open rows`() = runTest {
        val f = fixture()
        f.store.upsert(listOf(ItemMapper.toEntity(item("missing")), ItemMapper.toEntity(item("closed", StatusDto.RESOLVED))))
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        f.repo.refreshItem("a")
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        f.api.open = listOf(item())
        assertTrue(f.repo.refresh())
        assertEquals(listOf("a"), f.repo.openItems.first().map { it.id })
        assertNotNull(f.repo.item("closed").first())
        assertEquals(SyncState.Current(NOW), f.repo.syncState.value)
        assertEquals(NOW, f.store.lastSuccess())
    }

    @Test fun `final response prefetch detects concurrent closure without submitting`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.fetched = item(status = StatusDto.RESOLVED).copy(version = 2, responseChoice = "done")
        assertFalse(f.repo.answer("a", FinalAnswer.Choice("done")))
        assertEquals(0, f.api.submissions)
        assertEquals(StatusDto.RESOLVED, f.repo.item("a").first()!!.status)
    }

    @Test fun `final response persists server canonical result after exactly one submission`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.result = item(status = StatusDto.RESOLVED).copy(version = 2, responseText = "canonical", responseBy = ResponseByDto.PHONE)
        assertTrue(f.repo.answer("a", FinalAnswer.Text("reply")))
        assertEquals(listOf("open", "get:a", "text:a:reply"), f.api.calls)
        assertEquals("canonical", f.repo.item("a").first()!!.responseText)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun `ambiguous final response reconciles and never resubmits`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.writeError = ApiError.Transport("timeout")
        f.api.open = emptyList()
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(1, f.api.submissions)
        assertEquals(listOf("open", "get:a", "dismiss:a", "open"), f.api.calls)
        assertEquals(SyncState.Current(NOW), f.repo.syncState.value)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun `failed reconciliation leaves cache stale and blocks mutations`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.readError = ApiError.Transport("offline")
        assertFalse(f.repo.refresh())
        assertTrue(f.repo.syncState.value is SyncState.Stale)
        assertEquals(NOW, (f.repo.syncState.value as SyncState.Stale).lastSuccess)
        assertEquals(1, f.repo.openItems.first().size)
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(0, f.api.submissions)
    }

    @Test fun `check is optimistic then rolls back with one error`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeWrite = { release.await() }
        f.api.writeError = ApiError.Validation(null)
        val error = async { f.repo.errors.first() }
        val write = async { f.repo.setCheck("a", 0, true) }
        runCurrent()
        assertTrue(f.repo.item("a").first()!!.checks[0].done)
        release.complete(Unit)
        assertFalse(write.await())
        assertFalse(f.repo.item("a").first()!!.checks[0].done)
        assertTrue(error.await() is ApiError.Validation)
        assertEquals(1, f.api.submissions)
    }

    @Test fun `check completion uses canonical closure and version`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.result = item(status = StatusDto.RESOLVED).copy(version = 3, checks = listOf(CheckDto("step", true, "2026-09-04T10:00:00Z")))
        assertTrue(f.repo.setCheck("a", 0, true))
        assertEquals(3L, f.repo.item("a").first()!!.version)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun `history queries append and deduplicate membership without replacing queue or other cache`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val query = HistoryQuery(query = "first")
        f.api.history = HistoryPageDto(listOf(item("closed", StatusDto.RESOLVED)), "next")
        assertEquals("next", f.repo.refreshHistory(query).nextCursor)
        f.api.history = HistoryPageDto(listOf(item("closed", StatusDto.RESOLVED).copy(version = 2), item("other", StatusDto.DISMISSED)), null)
        f.repo.refreshHistory(query, "next")
        assertEquals(setOf("closed", "other"), f.repo.history(query).first().map { it.id }.toSet())
        f.api.history = HistoryPageDto(emptyList(), null)
        f.repo.refreshHistory(HistoryQuery(query = "second"))
        assertEquals(2, f.repo.history(query).first().size)
        f.repo.refreshHistory(query)
        assertTrue(f.repo.history(query).first().isEmpty())
        assertNotNull(f.repo.item("closed").first())
        assertEquals(listOf("a"), f.repo.openItems.first().map { it.id })
    }

    @Test fun `unpair invalidates in flight refresh and clears metadata and history`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeRead = { release.await() }
        val refresh = async { f.repo.refresh() }
        runCurrent()
        f.repo.unpair()
        assertNull(f.credentials.state.value)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertNull(f.store.lastSuccess())
        release.complete(Unit)
        assertFalse(refresh.await())
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun `unauthorized revokes pairing and clears cache`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.readError = ApiError.Unauthorized(null)
        assertFalse(f.repo.refresh())
        runCurrent()
        assertEquals(SyncState.Revoked, f.repo.syncState.value)
        assertNull(f.credentials.state.value)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun `cancelled optimistic write rolls back and disables mutations without swallowing cancellation`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.beforeWrite = { CompletableDeferred<Unit>().await() }
        val write = launch { f.repo.setCheck("a", 0, true) }
        runCurrent()
        assertTrue(f.repo.item("a").first()!!.checks[0].done)
        write.cancelAndJoin()
        assertFalse(f.repo.item("a").first()!!.checks[0].done)
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        assertTrue(write.isCancelled)
    }

    @Test fun `older open prefetch cannot permit answering a newer cached closure`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.store.upsert(listOf(ItemMapper.toEntity(item(status = StatusDto.RESOLVED).copy(version = 4))))
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(0, f.api.submissions)
        assertEquals(4L, f.repo.item("a").first()!!.version)
    }

    @Test fun `failed ambiguous reconciliation keeps controls disabled`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.api.beforeWrite = { f.api.readError = ApiError.Transport("offline") }
        f.api.writeError = ApiError.Transport("timeout")
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertTrue(f.repo.syncState.value is SyncState.Stale)
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        assertEquals(1, f.api.submissions)
    }

    @Test fun `pending check and refresh cannot overwrite newer canonical closure`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeWrite = { release.await() }
        f.api.result = item(status = StatusDto.RESOLVED).copy(version = 4)
        val write = async { f.repo.setCheck("a", 0, true) }
        runCurrent()
        val refresh = async { f.repo.refresh() }
        runCurrent()
        assertEquals(listOf("open", "check:a:0:true"), f.api.calls)
        release.complete(Unit)
        assertTrue(write.await())
        assertTrue(refresh.await())
        assertEquals(StatusDto.RESOLVED, f.repo.item("a").first()!!.status)
        assertEquals(4L, f.repo.item("a").first()!!.version)
    }

    @Test fun `unpair prevents pending check response from restoring data`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeWrite = { release.await() }
        val write = async { f.repo.setCheck("a", 0, true) }
        runCurrent()
        f.repo.unpair()
        release.complete(Unit)
        assertFalse(write.await())
        assertNull(f.repo.item("a").first())
        assertEquals(SyncState.Idle, f.repo.syncState.value)
    }

    @Test fun `external credential clear invalidates an in flight response`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeRead = { release.await() }
        val read = async { f.repo.refresh() }
        runCurrent()
        f.credentials.clear()
        runCurrent()
        assertTrue(f.repo.openItems.first().isEmpty())
        release.complete(Unit)
        assertFalse(read.await())
        assertEquals(SyncState.Idle, f.repo.syncState.value)
    }

    @Test fun `startup cached success never enables mutations and failed refresh retains timestamp`() = runTest {
        val store = MemoryStorage()
        store.reconcile(listOf(ItemMapper.toEntity(item())), NOW)
        val api = FakeApi().apply { readError = ApiError.Transport("offline") }
        val repo = DefaultItemRepository(store, { api }, FakeCredentials(), backgroundScope, Clock.fixed(NOW, ZoneOffset.UTC))
        runCurrent()
        assertFalse(repo.syncState.value.mutationsEnabled)
        assertEquals(1, repo.openItems.first().size)
        assertFalse(repo.answer("a", FinalAnswer.Dismiss))
        repo.refresh()
        assertEquals(NOW, (repo.syncState.value as SyncState.Stale).lastSuccess)
    }

    @Test fun `pairing metadata changes clear old account cache before refresh`() = runTest {
        val f = fixture()
        f.repo.refresh()
        f.credentials.save(PairedServer("https://other.example/", "other", "Phone"), "new")
        runCurrent()
        assertTrue(f.repo.openItems.first().isEmpty())
        assertNull(f.store.lastSuccess())
        assertFalse(f.repo.syncState.value.mutationsEnabled)
    }

    private fun TestScope.fixture(): Fixture {
        val store = MemoryStorage()
        val api = FakeApi()
        val credentials = FakeCredentials()
        return Fixture(store, api, credentials, DefaultItemRepository(store, { api }, credentials, backgroundScope, Clock.fixed(NOW, ZoneOffset.UTC)))
    }

    private data class Fixture(val store: MemoryStorage, val api: FakeApi, val credentials: FakeCredentials, val repo: DefaultItemRepository)

    companion object {
        val NOW: Instant = Instant.parse("2026-09-04T12:00:00Z")
        fun item(id: String = "a", status: StatusDto = StatusDto.OPEN) = ItemDto(
            id, null, "Title", "Body", "host", "project", PriorityDto.NORMAL,
            listOf("done"), listOf(CheckDto("step", false, null)), "why", "done", "https://example.com", status,
            null, null, null, "2026-09-04T09:00:00Z", if (status == StatusDto.RESOLVED) "2026-09-04T11:00:00Z" else null, null, 1,
        )
    }
}

private class FakeCredentials : CredentialStore {
    val state = MutableStateFlow<PairedServer?>(PairedServer("https://example.com/", "device", "Phone"))
    override fun observe() = state
    override suspend fun save(server: PairedServer, credential: String) { state.value = server }
    override suspend fun clear() { state.value = null }
}

private class FakeApi : LamApi {
    var open = listOf(ItemRepositoryTest.item())
    var fetched = ItemRepositoryTest.item()
    var result = ItemRepositoryTest.item(status = StatusDto.RESOLVED)
    var history = HistoryPageDto(emptyList(), null)
    var readError: ApiError? = null
    var writeError: ApiError? = null
    var beforeRead: suspend () -> Unit = {}
    var beforeWrite: suspend () -> Unit = {}
    val calls = mutableListOf<String>()
    var submissions = 0
    override suspend fun listOpenItems(): List<ItemDto> { calls += "open"; beforeRead(); readError?.let { throw it }; return open }
    override suspend fun getItem(id: String): ItemDto { calls += "get:$id"; readError?.let { throw it }; return fetched }
    override suspend fun getHistory(query: String?, priority: PriorityDto?, type: ItemTypeDto?, cursor: String?, limit: Int) = history
    private suspend fun write(call: String): ItemDto { calls += call; submissions++; beforeWrite(); writeError?.let { throw it }; return result }
    override suspend fun replyChoice(id: String, choice: String) = write("choice:$id:$choice")
    override suspend fun replyText(id: String, text: String) = write("text:$id:$text")
    override suspend fun dismiss(id: String) = write("dismiss:$id")
    override suspend fun setCheck(id: String, index: Int, done: Boolean) = write("check:$id:$index:$done")
    override suspend fun getDevice(): DeviceRegistrationDto = error("unused")
    override suspend fun updateDevice(update: DeviceUpdateDto): DeviceRegistrationDto = error("unused")
    override suspend fun revokeDevice(): DeviceSummaryDto = error("unused")
    override suspend fun claimPairing(sessionId: String, request: PairingClaimRequestDto): PairingClaimResponseDto = error("unused")
}

private class MemoryStorage : ItemStorage {
    private val rows = MutableStateFlow<Map<String, ItemEntity>>(emptyMap())
    private val members = MutableStateFlow<Map<String, Set<String>>>(emptyMap())
    private var success: Instant? = null
    override fun openItems() = rows.map { it.values.filter { row -> row.canonical.status == StatusDto.OPEN } }
    override fun item(id: String) = rows.map { it[id] }
    override fun history(key: String) = kotlinx.coroutines.flow.combine(rows, members) { all, membership -> membership[key].orEmpty().mapNotNull(all::get) }
    override suspend fun get(id: String) = rows.value[id]
    override suspend fun lastSuccess() = success
    override suspend fun upsert(items: List<ItemEntity>) {
        rows.value = rows.value.toMutableMap().apply {
            items.forEach { incoming ->
                val current = get(incoming.canonical.id)?.canonical
                val reopensClosed = current != null && current.status != StatusDto.OPEN &&
                    incoming.canonical.status == StatusDto.OPEN && incoming.canonical.version == current.version
                if ((current?.version ?: -1) <= incoming.canonical.version && !reopensClosed) {
                    put(incoming.canonical.id, incoming)
                }
            }
        }
    }
    override suspend fun reconcile(items: List<ItemEntity>, at: Instant) {
        upsert(items)
        val ids = items.map { it.canonical.id }.toSet()
        rows.value = rows.value.filterValues { it.canonical.status != StatusDto.OPEN || it.canonical.id in ids }
        success = at
    }
    override suspend fun cacheHistory(key: String, items: List<ItemEntity>, replace: Boolean) {
        upsert(items)
        members.value = members.value + (key to ((if (replace) emptySet() else members.value[key].orEmpty()) + items.map { it.canonical.id }))
    }
    override suspend fun optimistic(snapshot: ItemEntity, changed: ItemEntity) { rows.value = rows.value + (changed.canonical.id to changed) }
    override suspend fun rollback(snapshot: ItemEntity, tag: String) {
        if (rows.value[snapshot.canonical.id]?.optimisticTag == tag) rows.value = rows.value + (snapshot.canonical.id to snapshot)
    }
    override suspend fun clear() { rows.value = emptyMap(); members.value = emptyMap(); success = null }
}
