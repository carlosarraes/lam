package dev.carraes.lam.ui.requests

import dev.carraes.lam.items.*
import dev.carraes.lam.ui.agentColor
import dev.carraes.lam.ui.theme.MutedBurgundy
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class RequestsViewModelTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun loadingEmptyAndStaleRetainCachedItemsAndDisableActions() = runTest {
        val repo = RequestsFakeRepository()
        val vm = RequestsViewModel(repo, { repo.refresh() }, fixedClock)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertTrue(vm.state.value.loading)
        repo.syncState.value = SyncState.Current(now)
        runCurrent()
        assertFalse(vm.state.value.loading)
        assertTrue(vm.state.value.items.isEmpty())
        repo.openItems.value = listOf(request("cached"))
        repo.syncState.value = SyncState.Stale(now, Exception("offline"))
        runCurrent()
        assertEquals("cached", vm.state.value.items.single().id)
        assertEquals(now, vm.state.value.lastSuccess)
        assertFalse(vm.state.value.actionsEnabled)
        assertTrue(vm.state.value.stale)
        repo.openItems.value = emptyList()
        runCurrent()
        assertTrue(vm.state.value.stale)
        assertFalse(vm.state.value.loading)
    }

    @Test fun searchesTitleAgentAndBodyAndCombinesTypePriorityFilters() = runTest {
        val repo = RequestsFakeRepository()
        repo.openItems.value = listOf(request("low").copy(priority = PriorityDto.LOW),
            request("critical").copy(priority = PriorityDto.CRITICAL, body = "Unique deployment text"),
            request("check").copy(name = "Reviewer", checks = listOf(CheckDto("Run tests", false, null))))
        val vm = RequestsViewModel(repo, { true }, fixedClock)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertEquals(listOf("critical", "check", "low"), vm.state.value.items.map { it.id })
        for ((query, id) in listOf("CRITICAL" to "critical", "reviewer" to "check", " deployment " to "critical")) {
            vm.setQuery(query); runCurrent(); assertEquals(id, vm.state.value.items.single().id)
        }
        vm.setQuery(""); vm.setType(ItemTypeDto.CHECKLIST); vm.setPriority(PriorityDto.CRITICAL)
        runCurrent(); assertTrue(vm.state.value.items.isEmpty())
        vm.setPriority(null); runCurrent(); assertEquals("check", vm.state.value.items.single().id)
        vm.clearFilters(); runCurrent(); assertEquals(3, vm.state.value.items.size)
    }

    @Test fun sortsNewestWithinPriorityAndExcludesClosedItems() = runTest {
        val repo = RequestsFakeRepository()
        repo.openItems.value = listOf(request("old"), request("closed").copy(status = StatusDto.RESOLVED),
            request("new").copy(createdAt = "2026-09-04T11:59:00Z"))
        val vm = RequestsViewModel(repo, { true }, fixedClock)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertEquals(listOf("new", "old"), vm.state.value.items.map { it.id })
    }

    @Test fun cardsUseStableAgentColorsAndOnlyDecisionsWarnAboutMissingRecommendation() {
        val decision = request("decision")
        assertEquals(agentColor(decision), agentColor(decision.copy(id = "other")))
        assertEquals(agentColor(decision), agentColor(decision.copy(priority = PriorityDto.LOW)))
        assertEquals(MutedBurgundy, agentColor(decision.copy(priority = PriorityDto.CRITICAL)))
        assertTrue(decision.missingRecommendation)
        assertFalse(decision.copy(checks = listOf(CheckDto("Test", false, null))).missingRecommendation)
        assertFalse(decision.copy(recommendation = "Proceed").missingRecommendation)
        assertEquals("host:project", decision.copy(name = null).agentDisplay)
        assertEquals("5m", requestAge(decision, now))
    }

    @Test fun refreshUsesReconciliation() = runTest {
        val repo = RequestsFakeRepository()
        val vm = RequestsViewModel(repo, { repo.refresh() }, fixedClock)
        backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        vm.refresh(); runCurrent()
        assertEquals(1, repo.refreshes)
        assertTrue(vm.state.value.actionsEnabled)
    }

    @Test fun ageTicksWhileObservedAndStopsWithoutSubscribersWithoutNetworkPolling() = runTest {
        var instant = now
        var reads = 0
        val clock = object : Clock() {
            override fun getZone() = ZoneOffset.UTC
            override fun withZone(zone: java.time.ZoneId): Clock = this
            override fun instant(): Instant { reads++; return instant }
        }
        val repo = RequestsFakeRepository()
        val vm = RequestsViewModel(repo, { repo.refresh() }, clock)
        val observer = backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) { vm.state.collect() }
        runCurrent()
        assertEquals(now, vm.state.value.now)
        instant = now.plusSeconds(60)
        advanceTimeBy(60_000); runCurrent()
        assertEquals(instant, vm.state.value.now)
        observer.cancel(); runCurrent()
        val readCount = reads
        advanceTimeBy(300_000); runCurrent()
        assertEquals(readCount, reads)
        assertEquals(0, repo.refreshes)
    }
}

internal val now: Instant = Instant.parse("2026-09-04T12:00:00Z")
internal val fixedClock: Clock = Clock.fixed(now, ZoneOffset.UTC)
internal fun request(id: String) = Item(id, "Builder", id, "Body never previewed", "host", "project",
    PriorityDto.NORMAL, listOf("Approve", "Reject"), emptyList(), null, null, "", StatusDto.OPEN,
    null, null, null, "2026-09-04T11:55:00Z", null, null, 1)

internal class RequestsFakeRepository : ItemRepository {
    override val openItems = MutableStateFlow<List<Item>>(emptyList())
    override val syncState = MutableStateFlow<SyncState>(SyncState.Idle)
    override val errors = emptyFlow<Exception>()
    var refreshes = 0
    var gate: CompletableDeferred<Unit>? = null
    var succeeds = true
    override suspend fun refresh(): Boolean {
        refreshes++; syncState.value = SyncState.Refreshing; gate?.await()
        syncState.value = if (succeeds) SyncState.Current(now) else SyncState.Stale(now, Exception("offline"))
        return succeeds
    }
    override fun item(id: String) = openItems.map { items -> items.find { it.id == id } }
    override fun history(query: HistoryQuery) = flowOf(emptyList<Item>())
    override val cachedHistory = flowOf(emptyList<Item>())
    override suspend fun refreshItem(id: String) = false
    override suspend fun refreshHistory(query: HistoryQuery, cursor: String?) = HistoryResult(false, null)
    override suspend fun answer(id: String, answer: FinalAnswer) = false
    override suspend fun setCheck(id: String, index: Int, done: Boolean) = false
    override suspend fun unpair() = Unit
}
