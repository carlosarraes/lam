package dev.carraes.lam.items

import dev.carraes.lam.ui.detail.DecisionViewModel
import dev.carraes.lam.ui.requests.RequestsViewModel
import java.time.Clock
import java.time.ZoneOffset
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class PendingActionTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun movingBothWaysUpdatesCacheDetailAndBothLists() = runTest {
        val f = fixture()
        val clock = Clock.fixed(ItemRepositoryTest.NOW, ZoneOffset.UTC)
        val requests = RequestsViewModel(f.repo, f.repo::refresh, clock)
        val pending = RequestsViewModel(f.repo, f.repo::refresh, clock, pending = true)
        backgroundScope.launch { requests.state.collect() }
        backgroundScope.launch { pending.state.collect() }
        runCurrent()
        assertEquals(listOf("a"), requests.state.value.items.map { it.id })
        assertTrue(pending.state.value.items.isEmpty())

        f.api.result = f.api.fetched.copy(pendingAt = ItemRepositoryTest.NOW.toString(), version = 2)
        f.vm.togglePending()
        runCurrent()

        assertEquals(ItemRepositoryTest.NOW.toString(), f.store.get("a")?.canonical?.pendingAt)
        assertEquals(2L, f.vm.state.value.item?.version)
        assertTrue(requests.state.value.items.isEmpty())
        assertEquals(listOf("a"), pending.state.value.items.map { it.id })
        assertFalse(f.vm.state.value.movingPending)
        assertFalse(f.vm.state.value.pendingFailed)

        f.api.result = f.api.result.copy(pendingAt = null, version = 3)
        f.vm.togglePending()
        runCurrent()

        assertNull(f.store.get("a")?.canonical?.pendingAt)
        assertNull(f.vm.state.value.item?.pendingAt)
        assertEquals(listOf("a"), requests.state.value.items.map { it.id })
        assertTrue(pending.state.value.items.isEmpty())
        assertEquals(listOf("pending:a:true", "pending:a:false"), f.api.calls)
    }

    @Test fun duplicateTapsAreBlockedWhileMoveIsInFlight() = runTest {
        val f = fixture()
        val gate = CompletableDeferred<Unit>()
        f.api.beforeWrite = { gate.await() }
        f.api.result = f.api.fetched.copy(pendingAt = ItemRepositoryTest.NOW.toString(), version = 2)

        f.vm.togglePending()
        f.vm.togglePending()
        runCurrent()

        assertTrue(f.vm.state.value.movingPending)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertFalse(f.vm.state.value.replyEnabled)
        assertNull(f.store.get("a")?.canonical?.pendingAt)
        assertEquals(1, f.api.submissions)
        gate.complete(Unit)
        runCurrent()
        assertFalse(f.vm.state.value.movingPending)
        assertTrue(f.vm.state.value.actionsEnabled)
    }

    @Test fun rejectedMoveKeepsCacheAndReportsFailure() = runTest {
        val f = fixture()
        val error = ApiError.Validation("cannot move")
        val reported = backgroundScope.async(start = CoroutineStart.UNDISPATCHED) { f.repo.errors.first() }
        f.api.writeError = error

        f.vm.togglePending()
        runCurrent()

        assertSame(error, reported.await())
        assertNull(f.store.get("a")?.canonical?.pendingAt)
        assertTrue(f.vm.state.value.pendingFailed)
        assertFalse(f.vm.state.value.movingPending)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertEquals(listOf("pending:a:true"), f.api.calls)
    }

    @Test fun lostResponseRecoversCanonicalPendingStateWithoutReplayingWrite() = runTest {
        val f = fixture()
        f.api.beforeWrite = {
            f.api.fetched = f.api.fetched.copy(pendingAt = ItemRepositoryTest.NOW.toString(), version = 2)
            f.api.open = emptyList()
            f.api.pending = listOf(f.api.fetched)
        }
        f.api.writeError = ApiError.Transport("response lost")

        f.vm.togglePending()
        runCurrent()

        assertEquals(ItemRepositoryTest.NOW.toString(), f.store.get("a")?.canonical?.pendingAt)
        assertEquals(ItemRepositoryTest.NOW.toString(), f.vm.state.value.item?.pendingAt)
        assertTrue(f.vm.state.value.pendingFailed)
        assertTrue(f.vm.state.value.actionsEnabled)
        assertEquals(listOf("pending:a:true", "get:a", "open"), f.api.calls)
        assertEquals(1, f.api.submissions)
    }

    @Test fun closedItemsAndStaleSessionsCannotMove() = runTest {
        val f = fixture()
        f.api.fetched = f.api.fetched.copy(status = StatusDto.RESOLVED, version = 2)
        assertTrue(f.repo.refreshItem("a"))
        runCurrent()
        f.vm.togglePending()
        assertFalse(f.repo.setPending("a", true))
        runCurrent()
        assertEquals(0, f.api.submissions)

        f.api.fetched = f.api.fetched.copy(status = StatusDto.OPEN, version = 3)
        assertTrue(f.repo.refreshItem("a"))
        runCurrent()
        f.api.readError = ApiError.Transport("offline")
        assertFalse(f.repo.refresh())
        f.vm.togglePending()
        assertFalse(f.repo.setPending("a", true))
        runCurrent()
        assertEquals(0, f.api.submissions)
    }

    @Test fun unpairDiscardsLatePendingResponse() = runTest {
        val f = fixture()
        val gate = CompletableDeferred<Unit>()
        f.api.beforeWrite = { gate.await() }
        f.api.result = f.api.fetched.copy(pendingAt = ItemRepositoryTest.NOW.toString(), version = 2)
        val move = async { f.repo.setPending("a", true) }
        runCurrent()

        f.repo.unpair()
        gate.complete(Unit)
        assertFalse(move.await())
        assertNull(f.store.get("a"))
    }

    private suspend fun TestScope.fixture(): Fixture {
        val api = FakeApi()
        val store = MemoryStorage()
        val repo = DefaultItemRepository(store, { api }, FakeCredentials(), backgroundScope,
            Clock.fixed(ItemRepositoryTest.NOW, ZoneOffset.UTC))
        assertTrue(repo.refresh())
        val vm = DecisionViewModel("a", repo)
        runCurrent()
        api.calls.clear()
        return Fixture(api, store, repo, vm)
    }

    private data class Fixture(val api: FakeApi, val store: MemoryStorage,
        val repo: DefaultItemRepository, val vm: DecisionViewModel)
}
