package dev.carraes.lam.ui.detail

import dev.carraes.lam.items.*
import java.time.Clock
import java.time.ZoneOffset
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ChecklistViewModelTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun tickAndUntickUseOptimisticRepositoryStateWithoutReordering() = runTest {
        val f = fixture()
        val gate = CompletableDeferred<Unit>()
        f.api.beforeWrite = { gate.await() }
        f.vm.setCheck(0, true); f.vm.setCheck(0, true)
        runCurrent()
        assertEquals(listOf(true, false), f.vm.state.value.item!!.checks.map { it.done })
        assertEquals(listOf("first", "second"), f.vm.state.value.item!!.checks.map { it.label })
        assertTrue(f.vm.state.value.saving)
        gate.complete(Unit); runCurrent()
        assertEquals(1, f.api.submissions)
        f.vm.setCheck(0, false); runCurrent()
        assertEquals(listOf(false, false), f.vm.state.value.item!!.checks.map { it.done })
        assertEquals(2, f.api.submissions)
    }

    @Test fun rejectedTickRollsBackAndFeedbackCanBeConsumedOnlyOnce() = runTest {
        val f = fixture()
        f.api.writeError = ApiError.Validation("rejected")
        f.vm.setCheck(0, true); runCurrent()
        assertFalse(f.vm.state.value.item!!.checks[0].done)
        assertNotNull(f.vm.state.value.failure)
        val failure = f.vm.state.value.failure!!
        f.vm.consumeFailure(failure)
        assertNull(f.vm.state.value.failure)
        f.vm.consumeFailure(failure)
        assertNull(f.vm.state.value.failure)
        assertEquals(1, f.api.submissions)
    }

    @Test fun finalCheckKeepsCanonicalClosedDetailAndBlocksLateTap() = runTest {
        val f = fixture()
        f.vm.setCheck(0, true); runCurrent()
        f.vm.setCheck(1, true); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertTrue(f.repo.openItems.first().isEmpty())
        f.vm.setCheck(1, false); runCurrent()
        assertEquals(2, f.api.submissions)
    }

    @Test fun lostFinalCheckResponseRecoversCanonicalClosureWithoutReplaying() = runTest {
        val f = fixture()
        f.vm.setCheck(0, true); runCurrent()
        f.api.afterWriteError = ApiError.Transport("response lost")
        f.vm.setCheck(1, true); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals(listOf(true, true), f.vm.state.value.item!!.checks.map { it.done })
        assertFalse(f.vm.state.value.actionsEnabled)
        assertEquals(2, f.api.submissions)
        assertTrue(f.repo.openItems.first().isEmpty())
    }

    @Test fun staleStateKeepsChecklistReadableAndBlocksMutationAndQuickActions() = runTest {
        val repo = DetailFakeRepository().apply { current.value = detailItem.copy(checks = listOf(CheckDto("Read me", false, null))) }
        val checks = ChecklistViewModel("request", repo)
        val decision = DecisionViewModel("request", repo)
        runCurrent()
        repo.syncState.value = SyncState.Stale(detailNow, Exception("offline"))
        runCurrent()
        checks.setCheck(0, true); decision.quickResponse(); decision.dismissRequest()
        assertEquals("Read me", checks.state.value.item!!.checks.single().label)
        assertFalse(checks.state.value.actionsEnabled)
        assertFalse(checks.state.value.saving)
        assertFalse(decision.state.value.quickOpen)
        assertNull(decision.state.value.confirmation)
    }

    @Test fun failedRecoveryKeepsRolledBackSnapshotStaleUntilTargetReadSucceeds() = runTest {
        val f = fixture()
        f.vm.setCheck(0, true); runCurrent()
        f.api.afterWriteError = ApiError.Transport("response lost")
        f.api.beforeGet = { throw ApiError.Transport("read offline") }
        f.vm.setCheck(1, true); runCurrent()
        assertEquals(StatusDto.OPEN, f.vm.state.value.item?.status)
        assertEquals(listOf(true, false), f.vm.state.value.item!!.checks.map { it.done })
        assertTrue(f.vm.state.value.sync is SyncState.Stale)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertEquals(2, f.api.submissions)
        f.api.beforeGet = {}
        assertTrue(f.repo.refresh()); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals(2, f.api.submissions)
    }

    @Test fun quickResponseAndDismissalCloseOnRemoteClosureAndCannotReplay() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.quickResponse()
        assertTrue(vm.state.value.quickOpen)
        vm.dismissRequest()
        val old = vm.state.value.confirmation!!
        assertEquals(FinalAnswer.Dismiss, old.answer)
        assertTrue(repo.answers.isEmpty())
        repo.current.value = detailItem.copy(status = StatusDto.DISMISSED, version = 2)
        runCurrent()
        assertFalse(vm.state.value.quickOpen)
        assertNull(vm.state.value.confirmation)
        vm.confirm(old); runCurrent()
        assertTrue(repo.answers.isEmpty())
    }

    @Test fun dismissalNeedsFreshConfirmationAndConsumesDoubleTap() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.dismissRequest()
        val cancelled = vm.state.value.confirmation!!
        vm.dismissConfirmation(); vm.confirm(cancelled)
        assertTrue(repo.answers.isEmpty())
        vm.dismissRequest()
        val valid = vm.state.value.confirmation!!
        vm.confirm(valid); vm.confirm(valid); runCurrent()
        assertEquals(listOf(FinalAnswer.Dismiss), repo.answers)
    }

    @Test fun cancellingPlainQuickReplyClosesTheQuickResponseSession() = runTest {
        val repo = DetailFakeRepository().apply { current.value = detailItem.copy(choices = emptyList()) }
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.quickResponse()
        assertTrue(vm.state.value.replyOpen)
        vm.dismissReply()
        assertFalse(vm.state.value.replyOpen)
        assertFalse(vm.state.value.quickOpen)
        assertTrue(repo.answers.isEmpty())
    }

    private suspend fun TestScope.fixture(): Fixture {
        val api = ChecklistApi()
        val repo = DefaultItemRepository(MemoryStorage(), { api }, FakeCredentials(), backgroundScope,
            Clock.fixed(ItemRepositoryTest.NOW, ZoneOffset.UTC))
        assertTrue(repo.refresh())
        val vm = ChecklistViewModel("a", repo)
        runCurrent()
        return Fixture(api, repo, vm)
    }
    private data class Fixture(val api: ChecklistApi, val repo: DefaultItemRepository, val vm: ChecklistViewModel)
}

private class ChecklistApi : LamApi by FakeApi() {
    var value = ItemRepositoryTest.item().copy(checks = listOf(CheckDto("first", false, null), CheckDto("second", false, null)))
    var beforeWrite: suspend () -> Unit = {}
    var writeError: ApiError? = null
    var afterWriteError: ApiError? = null
    var beforeGet: suspend () -> Unit = {}
    var submissions = 0
    override suspend fun setCheck(id: String, index: Int, done: Boolean): ItemDto {
        submissions++
        beforeWrite()
        writeError?.let { throw it }
        val checks = value.checks.mapIndexed { i, check -> if (i == index) check.copy(done = done) else check }
        value = value.copy(checks = checks, status = if (checks.all { it.done }) StatusDto.RESOLVED else StatusDto.OPEN, version = value.version + 1)
        afterWriteError?.let { throw it }
        return value
    }
    override suspend fun getItem(id: String): ItemDto { beforeGet(); return value }
    override suspend fun listOpenItems() = listOf(value).filter { it.status == StatusDto.OPEN }
    override suspend fun replyChoice(id: String, choice: String): ItemDto = error("unused")
    override suspend fun replyText(id: String, text: String): ItemDto = error("unused")
    override suspend fun dismiss(id: String): ItemDto = error("unused")
}
