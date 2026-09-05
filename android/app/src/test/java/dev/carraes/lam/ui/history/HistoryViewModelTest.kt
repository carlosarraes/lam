package dev.carraes.lam.ui.history

import dev.carraes.lam.items.*
import dev.carraes.lam.items.ItemRepositoryTest.Companion.item
import dev.carraes.lam.items.ItemRepositoryTest.Companion.NOW
import dev.carraes.lam.ui.requests.fixedClock
import androidx.lifecycle.*
import dev.carraes.lam.sync.LifecycleReconciler
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class HistoryViewModelTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun completedReconciliationWhileHistoryIsLoadingQueuesOneVisibleRefresh() = runTest {
        val api = FakeApi()
        val repo = DefaultItemRepository(MemoryStorage(), { api }, FakeCredentials(), backgroundScope, fixedClock)
        repo.refresh()
        val completions = MutableStateFlow(0L)
        val gate = CompletableDeferred<Unit>()
        var reads = 0
        api.historyRead = { query, cursor ->
            reads++
            assertEquals(HistoryQuery(), query)
            assertNull(cursor)
            if (reads == 1) gate.await()
            HistoryPageDto(listOf(item("fresh", StatusDto.RESOLVED)), null)
        }
        val vm = HistoryViewModel(repo, completions)
        vm.setVisible(true); runCurrent()
        completions.value++; runCurrent(); completions.value++; runCurrent()
        assertEquals(1, reads)
        gate.complete(Unit); runCurrent()
        assertEquals(2, reads)
        assertEquals(listOf("fresh"), vm.state.value.items.map { it.id })
        assertFalse(vm.state.value.loading)
    }

    @Test fun visibleHistoryRefreshesOnceOnResumeWithoutPollingOrSyncFeedback() = runTest {
        val api = FakeApi()
        val repo = DefaultItemRepository(MemoryStorage(), { api }, FakeCredentials(), backgroundScope, fixedClock)
        val owner = object : LifecycleOwner {
            override val lifecycle = LifecycleRegistry.createUnsafe(this)
        }
        val reconciler = LifecycleReconciler(repo, repo.reconciliationSession, owner.lifecycle, backgroundScope)
        owner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_START); runCurrent()
        var historyReads = 0
        api.historyRead = { _, _ -> historyReads++; api.history }
        val vm = HistoryViewModel(repo, reconciler.completedReconciliations)
        vm.setVisible(true)
        runCurrent()
        assertEquals(1, historyReads)
        owner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_STOP); runCurrent()
        api.history = HistoryPageDto(listOf(item("new", StatusDto.RESOLVED)), null)
        owner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_START); runCurrent()
        assertEquals(listOf("new"), vm.state.value.items.map { it.id })
        assertEquals(2, historyReads)
        vm.setVisible(false)
        owner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_STOP); runCurrent()
        owner.lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_START); runCurrent()
        assertEquals(2, historyReads)
        advanceTimeBy(600_000); runCurrent()
        assertEquals(2, historyReads)
        reconciler.close()
    }

    @Test fun `new offline queries and filters search all cached closed rows with incomplete label`() = runTest {
        val storage = MemoryStorage()
        val api = FakeApi()
        val repo = DefaultItemRepository(storage, { api }, FakeCredentials(), backgroundScope, fixedClock)
        repo.refresh()
        val cached = listOf(
            item("critical", StatusDto.RESOLVED).copy(title = "Deploy", body = "Unique body", name = "Builder", priority = PriorityDto.CRITICAL, checks = emptyList()),
            item("check", StatusDto.RESOLVED).copy(title = "Verify", name = "Reviewer", priority = PriorityDto.LOW),
            item("plain", StatusDto.RESOLVED).copy(title = "Note", choices = emptyList(), checks = emptyList()),
        )
        api.historyRead = { _, _ -> HistoryPageDto(cached, null) }
        val vm = HistoryViewModel(repo)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertFalse(vm.state.value.incomplete)
        api.historyRead = { _, _ -> throw ApiError.Transport("offline") }
        for ((query, id) in listOf("Deploy" to "critical", "unique body" to "critical", "Reviewer" to "check")) {
            vm.setQuery(query); runCurrent()
            assertEquals(id, vm.state.value.items.single().id)
            assertTrue(vm.state.value.incomplete)
        }
        vm.setQuery("HOST:PROJECT"); runCurrent()
        assertEquals(setOf("critical", "check", "plain"), vm.state.value.items.map { it.id }.toSet())
        vm.setPriority(PriorityDto.CRITICAL); vm.setType(ItemTypeDto.CHOICE); runCurrent()
        assertEquals("critical", vm.state.value.items.single().id)
        vm.setPriority(null); vm.setType(ItemTypeDto.CHECKLIST); runCurrent()
        assertEquals("check", vm.state.value.items.single().id)
        vm.setType(ItemTypeDto.PLAIN); runCurrent()
        assertEquals("plain", vm.state.value.items.single().id)
        api.historyRead = { query, cursor ->
            assertEquals(HistoryQuery("HOST:PROJECT", type = ItemTypeDto.PLAIN), query)
            assertNull(cursor)
            HistoryPageDto(listOf(item("remote", StatusDto.RESOLVED).copy(choices = emptyList(), checks = emptyList())), "opaque")
        }
        vm.refresh(); runCurrent()
        assertEquals("remote", vm.state.value.items.single().id)
        assertFalse(vm.state.value.incomplete)
        assertEquals("opaque", vm.state.value.nextCursor)
    }

    @Test fun `server search and filters replace results while opaque cursor appends tied closure rows`() = runTest {
        val storage = MemoryStorage()
        val api = FakeApi()
        api.open = listOf(item("open"))
        val repo = DefaultItemRepository(storage, { api }, FakeCredentials(), backgroundScope, fixedClock)
        repo.refresh()
        val calls = mutableListOf<Pair<HistoryQuery, String?>>()
        api.historyRead = { query, cursor ->
            calls += query to cursor
            when {
                query.query == "remote body" -> HistoryPageDto(listOf(item("match", StatusDto.RESOLVED)), null)
                query.priority != null -> HistoryPageDto(listOf(item("priority", StatusDto.RESOLVED)), null)
                query.type != null -> HistoryPageDto(listOf(item("type", StatusDto.RESOLVED)), null)
                cursor == "opaque+/==" -> HistoryPageDto(listOf(item("a", StatusDto.RESOLVED)), null)
                else -> HistoryPageDto(listOf(item("z", StatusDto.RESOLVED)), "opaque+/==")
            }
        }
        val vm = HistoryViewModel(repo)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertEquals(listOf("z"), vm.state.value.items.map { it.id })
        vm.loadMore(); runCurrent()
        assertEquals(listOf("z", "a"), vm.state.value.items.map { it.id })
        assertEquals("opaque+/==", calls.last().second)
        vm.setQuery("remote body"); runCurrent()
        assertEquals(listOf("match"), vm.state.value.items.map { it.id })
        assertEquals(HistoryQuery("remote body") to null, calls.last())
        vm.setQuery(""); vm.setPriority(PriorityDto.CRITICAL); runCurrent()
        assertEquals(HistoryQuery(null, PriorityDto.CRITICAL) to null, calls.last())
        assertEquals(listOf("priority"), vm.state.value.items.map { it.id })
        vm.setPriority(null); vm.setType(ItemTypeDto.CHECKLIST); runCurrent()
        assertEquals(HistoryQuery(type = ItemTypeDto.CHECKLIST) to null, calls.last())
        assertEquals(listOf("type"), vm.state.value.items.map { it.id })
        assertEquals(listOf("open"), repo.openItems.first().map { it.id })
    }

    @Test fun `failed read shows cached history and preserves cursor for retry`() = runTest {
        val storage = MemoryStorage()
        val api = FakeApi()
        val repo = DefaultItemRepository(storage, { api }, FakeCredentials(), backgroundScope, fixedClock)
        repo.refresh()
        api.historyRead = { _, _ -> HistoryPageDto(listOf(item("cached", StatusDto.RESOLVED)), "next") }
        val vm = HistoryViewModel(repo)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        api.historyRead = { _, _ -> throw ApiError.Transport("offline") }
        vm.loadMore(); runCurrent()
        assertEquals("cached", vm.state.value.items.single().id)
        assertTrue(vm.state.value.stale)
        assertTrue(vm.state.value.loadFailed)
        assertEquals("next", vm.state.value.nextCursor)
        assertFalse(repo.syncState.value.mutationsEnabled)
        api.historyRead = { _, cursor ->
            assertEquals("next", cursor)
            HistoryPageDto(listOf(item("older", StatusDto.RESOLVED).copy(resolvedAt = "2026-09-03T11:00:00Z")), null)
        }
        vm.loadMore(); runCurrent()
        assertEquals(listOf("cached", "older"), vm.state.value.items.map { it.id })
        assertFalse(repo.syncState.value.mutationsEnabled)
    }
}
