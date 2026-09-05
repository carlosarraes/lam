package dev.carraes.lam.items

import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import dev.carraes.lam.sync.LifecycleReconciler
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.LifecycleRegistry
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.joinAll
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ItemRepositoryTest {
    @Test fun `self revoke clears cache and credentials only after remote success`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val held = CompletableDeferred<Unit>()
        f.api.beforeRevoke = { held.await() }
        val revoke = async { f.repo.revoke(requireNotNull(f.repo.reconciliationSession.value)) }
        runCurrent()
        assertNotNull(f.credentials.state.value)
        assertEquals(1, f.repo.openItems.first().size)
        held.complete(Unit)
        assertEquals(UnpairResult.REVOKED, revoke.await())
        assertNull(f.credentials.state.value)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertNull(f.store.lastSuccess())
        assertEquals(1, f.api.revocations)
    }

    @Test fun `unavailable revoke preserves pairing until explicit guarded local erase`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val session = requireNotNull(f.repo.reconciliationSession.value)
        f.api.revokeError = ApiError.Transport("offline secret")
        assertEquals(UnpairResult.UNAVAILABLE, f.repo.revoke(session))
        assertNotNull(f.credentials.state.value)
        assertEquals(1, f.repo.openItems.first().size)
        assertTrue(f.repo.eraseLocal(session))
        assertNull(f.credentials.state.value)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertEquals(1, f.api.revocations)
    }

    @Test fun `old confirmations and in flight revoke cannot erase same metadata replacement`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val original = requireNotNull(f.credentials.state.value)
        val session = requireNotNull(f.repo.reconciliationSession.value)
        val held = CompletableDeferred<Unit>()
        f.api.beforeRevoke = { held.await() }
        val revoke = async { f.repo.revoke(session) }
        runCurrent()
        f.repo.credentialStore.save(original, "replacement credential")
        held.complete(Unit)
        assertEquals(UnpairResult.SESSION_CHANGED, revoke.await())
        assertFalse(f.repo.eraseLocal(session))
        assertEquals(UnpairResult.SESSION_CHANGED, f.repo.revoke(session))
        assertEquals(original, f.credentials.state.value)
        assertEquals(1, f.api.revocations)
    }

    @Test fun `session identity is unavailable during restore and replacement and changes with identical metadata`() = runTest {
        val f = fixture()
        assertNull(f.repo.reconciliationSession.value)
        runCurrent()
        assertNotNull(f.repo.reconciliationSession.value)
        val originalSession = f.repo.reconciliationSession.value
        val originalServer = requireNotNull(f.credentials.state.value)
        val saved = CompletableDeferred<Unit>()
        f.credentials.beforeSave = { saved.await() }
        val saving = async { f.repo.credentialStore.save(originalServer, "synthetic-replacement") }
        runCurrent()
        assertNull("no session may be used while replacement persistence is pending", f.repo.reconciliationSession.value)
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        saved.complete(Unit)
        saving.await()
        assertNotNull(f.repo.reconciliationSession.value)
        val replacementSession = f.repo.reconciliationSession.value
        assertNotEquals(originalSession, replacementSession)
        f.repo.unpair()
        assertNull(f.repo.reconciliationSession.value)
    }

    @Test fun `replacement pairing gets its own refresh while previous session is suspended`() = runTest {
        assertReplacementReconciles(sameMetadata = false, unpair = true)
    }

    @Test fun `same metadata replacement refresh starts before the observer consumes the save`() = runTest {
        assertReplacementReconciles(sameMetadata = true, unpair = false)
    }

    private suspend fun TestScope.assertReplacementReconciles(sameMetadata: Boolean, unpair: Boolean) {
        val f = fixture()
        val lifecycleOwner = object : LifecycleOwner {
            override val lifecycle = LifecycleRegistry.createUnsafe(this)
        }
        val held = CompletableDeferred<Unit>()
        f.api.beforeRead = { held.await() }
        val reconciler = LifecycleReconciler(f.repo, f.repo.reconciliationSession, lifecycleOwner.lifecycle, backgroundScope)
        lifecycleOwner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_START)
        runCurrent()
        assertEquals(listOf("open"), f.api.calls)
        val original = requireNotNull(f.credentials.state.value)
        if (unpair) f.repo.unpair()
        val replacement = if (sameMetadata) original else original.copy(deviceId = "device-b")
        f.repo.credentialStore.save(replacement, "synthetic-replacement")
        // Start the pairing callback without yielding to the lifecycle collector after save.
        f.api.beforeRead = {}
        f.api.open = listOf(item("session-b"))
        val pairingRefresh = async(start = CoroutineStart.UNDISPATCHED) { reconciler.refresh() }
        runCurrent()
        assertTrue("B must complete without waiting for A's suspended transport", pairingRefresh.isCompleted)
        held.complete(Unit)
        assertTrue("B must reconcile itself instead of inheriting A's invalidated result", pairingRefresh.await())
        assertEquals(SyncState.Current(NOW), f.repo.syncState.value)
        assertEquals(listOf("session-b"), f.repo.openItems.first().map { it.id })
        assertEquals(listOf("open", "open"), f.api.calls)
        f.credentials.publishCurrent()
        runCurrent()
        assertEquals("delayed metadata must not start another refresh", 2, f.api.calls.size)
        reconciler.close()
    }

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
        f.api.beforeWrite = { f.api.fetched = item(status = StatusDto.DISMISSED).copy(version = 2) }
        f.api.open = emptyList()
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(1, f.api.submissions)
        assertEquals(listOf("open", "get:a", "dismiss:a", "get:a", "open"), f.api.calls)
        assertEquals(SyncState.Current(NOW), f.repo.syncState.value)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertEquals(StatusDto.DISMISSED, f.repo.item("a").first()?.status)
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

    @Test fun `credential replacement blocks immediate mutation while its metadata observer is deferred`() = runTest {
        assertCredentialReplacement(PairedServer("https://other.example/", "other", "Other phone"))
    }

    @Test fun `same metadata credential replacement requires a new complete reconciliation`() = runTest {
        assertCredentialReplacement(PairedServer("https://example.com/", "device", "Phone"))
    }

    @Test fun `queued credential replacements finish with a suspending metadata publisher`() = runTest {
        assertQueuedCredentialTransitions(clearLast = false)
    }

    @Test fun `queued credential replacements followed by unpair finish without restoring a prior pairing`() = runTest {
        assertQueuedCredentialTransitions(clearLast = true)
    }

    private suspend fun TestScope.assertQueuedCredentialTransitions(clearLast: Boolean) {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        var saves = 0
        f.credentials.beforeSave = { if (saves++ == 0) release.await() }
        val servers = (1..3).map { PairedServer("https://example.com/", "device-$it", "Phone $it") }
        val writes = servers.mapIndexed { index, server ->
            backgroundScope.launch(start = CoroutineStart.UNDISPATCHED) {
                if (clearLast && index == 2) f.repo.credentialStore.clear()
                else f.repo.credentialStore.save(server, "credential-$index")
            }
        }
        release.complete(Unit)
        val completed = withTimeoutOrNull(1_000) { writes.joinAll(); true }
        if (completed == null) {
            // Removing the test subscriber releases a blocked emit even inside NonCancellable teardown.
            backgroundScope.cancel()
            writes.joinAll()
        }
        assertEquals("credential transitions must complete without a flow/transition-lock cycle", true, completed)
        runCurrent()
        val expected = if (clearLast) null else servers.last()
        assertEquals(expected, f.credentials.state.value)
        assertEquals(expected, f.repo.credentialStore.observe().first())
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        if (!clearLast) assertTrue(f.repo.refresh())
    }

    @Test fun `managed pairing stream publishes the replacement without waiting for raw metadata delivery`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val observed = mutableListOf<PairedServer?>()
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) {
            f.repo.credentialStore.observe().collect { observed += it }
        }
        f.credentials.publishChanges = false
        val replacement = PairedServer("https://other.example/", "other", "Phone")
        f.repo.credentialStore.save(replacement, "replacement")
        assertEquals(replacement, observed.last())
        assertFalse(f.repo.syncState.value.mutationsEnabled)
    }

    @Test fun `old bearer rejection after replacement cannot remove the new credential`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeRead = { release.await() }
        val oldRead = async { f.repo.refresh() }
        runCurrent()
        val replacement = PairedServer("https://other.example/", "other", "Phone")
        f.repo.credentialStore.save(replacement, "replacement")
        f.api.readError = ApiError.Unauthorized(null)
        release.complete(Unit)
        assertFalse(oldRead.await())
        assertEquals(replacement, f.credentials.state.value)
        assertEquals(SyncState.Idle, f.repo.syncState.value)
    }

    @Test fun `unpair preserves both cache and credential cleanup errors while disabling the session`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val cacheFailure = IllegalStateException("cache failed")
        val credentialFailure = IllegalStateException("credential failed")
        f.store.clearError = cacheFailure
        f.credentials.clearError = credentialFailure
        val failure = runCatching { f.repo.unpair() }.exceptionOrNull()
        assertNotNull(failure)
        assertEquals(listOf(cacheFailure, credentialFailure), failure!!.suppressed.toList())
        assertNull(f.credentials.state.value)
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
    }

    private suspend fun TestScope.assertCredentialReplacement(server: PairedServer) {
        val f = fixture()
        assertTrue(f.repo.refresh())
        f.credentials.publishChanges = false
        f.repo.credentialStore.save(server, "replacement")
        assertEquals(server, f.credentials.state.value)
        assertFalse("replacement must disable controls before metadata collection", f.repo.syncState.value.mutationsEnabled)
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(0, f.api.submissions)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertTrue(f.repo.refresh())
        f.credentials.publishCurrent()
        runCurrent()
        assertTrue("delayed metadata must not invalidate the reconciled replacement", f.repo.syncState.value.mutationsEnabled)
        assertTrue(f.repo.answer("a", FinalAnswer.Dismiss))
        assertEquals(1, f.api.submissions)
    }

    @Test fun `unpair removes credentials and invalidates late response when cache cleanup fails`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val release = CompletableDeferred<Unit>()
        f.api.beforeRead = { release.await() }
        val read = async { f.repo.refresh() }
        runCurrent()
        val cacheFailure = IllegalStateException("database unavailable")
        f.store.clearError = cacheFailure
        val failure = runCatching { f.repo.unpair() }.exceptionOrNull()
        assertNotNull(failure)
        assertNull("credential removal must still be attempted", f.credentials.state.value)
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        assertTrue("cleanup failure must retain its cause", failure!!.suppressed.contains(cacheFailure))
        release.complete(Unit)
        assertFalse(read.await())
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        runCurrent()
        assertEquals(SyncState.Idle, f.repo.syncState.value)
    }

    @Test fun `bearer rejection removes credentials and stays revoked when cache cleanup fails`() = runTest {
        val f = fixture()
        f.repo.refresh()
        val cacheFailure = IllegalStateException("database unavailable")
        f.store.clearError = cacheFailure
        f.api.readError = ApiError.Unauthorized(null)
        val error = async { f.repo.errors.first() }
        assertFalse(f.repo.refresh())
        assertNull(f.credentials.state.value)
        assertEquals(SyncState.Revoked, f.repo.syncState.value)
        assertTrue(error.await().suppressed.contains(cacheFailure))
        runCurrent()
        assertEquals(SyncState.Revoked, f.repo.syncState.value)
        assertFalse(f.repo.answer("a", FinalAnswer.Dismiss))
        assertFalse(f.repo.refresh())
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

internal class FakeCredentials : CredentialStore {
    val state = MutableStateFlow<PairedServer?>(PairedServer("https://example.com/", "device", "Phone"))
    private val published = MutableSharedFlow<PairedServer?>(replay = 1).apply { tryEmit(state.value) }
    var publishChanges = true
    var clearError: Exception? = null
    var beforeSave: suspend () -> Unit = {}
    override fun observe() = published
    suspend fun publishCurrent() { published.emit(state.value) }
    override suspend fun save(server: PairedServer, credential: String) {
        beforeSave()
        state.value = server
        if (publishChanges) publishCurrent()
    }
    override suspend fun clear() {
        state.value = null
        if (publishChanges) publishCurrent()
        clearError?.let { throw it }
    }
}

internal class FakeApi : LamApi {
    var beforeRevoke: suspend () -> Unit = {}
    var revokeError: ApiError? = null
    var revocations = 0
    var historyRead: (suspend (HistoryQuery, String?) -> HistoryPageDto)? = null
    var open = listOf(ItemRepositoryTest.item())
    var fetched = ItemRepositoryTest.item()
    var result = ItemRepositoryTest.item(status = StatusDto.RESOLVED)
    var history = HistoryPageDto(emptyList(), null)
    var readError: ApiError? = null
    var writeError: ApiError? = null
    var beforeRead: suspend () -> Unit = {}
    var beforeGet: suspend () -> Unit = {}
    var beforeWrite: suspend () -> Unit = {}
    val calls = mutableListOf<String>()
    var submissions = 0
    override suspend fun listOpenItems(): List<ItemDto> { calls += "open"; beforeRead(); readError?.let { throw it }; return open }
    override suspend fun getItem(id: String): ItemDto { calls += "get:$id"; beforeGet(); readError?.let { throw it }; return fetched }
    override suspend fun getHistory(query: String?, priority: PriorityDto?, type: ItemTypeDto?, cursor: String?, limit: Int) =
        historyRead?.invoke(HistoryQuery(query, priority, type), cursor) ?: history
    private suspend fun write(call: String): ItemDto { calls += call; submissions++; beforeWrite(); writeError?.let { throw it }; return result }
    override suspend fun replyChoice(id: String, choice: String) = write("choice:$id:$choice")
    override suspend fun replyText(id: String, text: String) = write("text:$id:$text")
    override suspend fun dismiss(id: String) = write("dismiss:$id")
    override suspend fun setCheck(id: String, index: Int, done: Boolean) = write("check:$id:$index:$done")
    override suspend fun getDevice(): DeviceRegistrationDto = error("unused")
    override suspend fun updateDevice(update: DeviceUpdateDto): DeviceRegistrationDto = error("unused")
    override suspend fun revokeDevice(): DeviceSummaryDto {
        revocations++; beforeRevoke(); revokeError?.let { throw it }
        return DeviceSummaryDto("device", "Phone", "1", "16", "2026-09-04T09:00:00Z", null, false, "2026-09-04T12:00:00Z")
    }
    override suspend fun claimPairing(sessionId: String, request: PairingClaimRequestDto): PairingClaimResponseDto = error("unused")
}

internal class MemoryStorage : ItemStorage {
    private val rows = MutableStateFlow<Map<String, ItemEntity>>(emptyMap())
    private val members = MutableStateFlow<Map<String, Set<String>>>(emptyMap())
    private var success: Instant? = null
    var clearError: Exception? = null
    override fun openItems() = rows.map { it.values.filter { row -> row.canonical.status == StatusDto.OPEN } }
    override fun item(id: String) = rows.map { it[id] }
    override fun history(key: String) = kotlinx.coroutines.flow.combine(rows, members) { all, membership -> membership[key].orEmpty().mapNotNull(all::get) }
    override suspend fun get(id: String) = rows.value[id]
    override suspend fun lastSuccess() = success
    override suspend fun counts() = dev.carraes.lam.diagnostics.ItemCounts(rows.value.size, rows.value.values.count { it.canonical.status == StatusDto.OPEN })
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
    override suspend fun clear() {
        clearError?.let { throw it }
        rows.value = emptyMap(); members.value = emptyMap(); success = null
    }
}
