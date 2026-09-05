package dev.carraes.lam.ui.detail

import dev.carraes.lam.items.*
import java.time.Instant
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class DecisionViewModelTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun everyChoiceNeedsConfirmationAndDoubleConfirmationOnlySendsOnce() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent()
        for (choice in listOf("Approve", "Reject")) {
            vm.choose(choice)
            assertEquals(FinalAnswer.Choice(choice), vm.state.value.confirmation?.answer)
            assertTrue(repo.answers.isEmpty())
            vm.dismissConfirmation()
        }
        vm.choose("Approve")
        val confirmation = vm.state.value.confirmation!!
        repo.answerGate = CompletableDeferred()
        vm.confirm(confirmation); vm.confirm(confirmation)
        runCurrent()
        assertEquals(listOf(FinalAnswer.Choice("Approve")), repo.answers)
        assertTrue(vm.state.value.submitting)
        assertFalse(vm.state.value.actionsEnabled)
        repo.answerGate!!.complete(Unit)
        runCurrent()
        assertFalse(vm.state.value.submitting)
        assertEquals(StatusDto.RESOLVED, vm.state.value.item?.status)
    }

    @Test fun multilineReplyUsesExactUtf8BoundaryAndPreservesText() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent()
        vm.writeReply()
        vm.editReply("\n  ")
        assertFalse(vm.state.value.replyValid)
        vm.editReply("é".repeat(4096))
        assertEquals(8192, vm.state.value.replyBytes)
        assertTrue(vm.state.value.replyValid)
        vm.editReply("é".repeat(4096) + "a")
        vm.reviewReply()
        assertNull(vm.state.value.confirmation)
        assertFalse(vm.state.value.replyValid)
        val text = "Proceed\n\nKeep the fallback. "
        vm.editReply(text)
        vm.reviewReply()
        assertTrue(repo.answers.isEmpty())
        vm.confirm(vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(listOf(FinalAnswer.Text(text)), repo.answers)
        assertEquals(text, vm.state.value.item?.responseText)
        assertFalse(vm.state.value.replyOpen)
    }

    @Test fun remoteClosureKeepsCanonicalOutcomeAndInvalidatesOldSheetCallback() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent()
        vm.writeReply(); vm.editReply("Old draft"); vm.choose("Approve")
        val old = vm.state.value.confirmation!!
        repo.current.value = detailItem.copy(status = StatusDto.RESOLVED, responseChoice = "Reject", responseBy = ResponseByDto.CLI, version = 2)
        runCurrent()
        assertEquals("Reject", vm.state.value.item?.responseChoice)
        assertEquals(ResponseByDto.CLI, vm.state.value.item?.responseBy)
        assertNull(vm.state.value.confirmation)
        assertFalse(vm.state.value.replyOpen)
        vm.confirm(old); runCurrent()
        assertTrue(repo.answers.isEmpty())
    }

    @Test fun updatedItemAndStaleSyncInvalidateSelectionsEvenIfConnectivityReturns() = runTest {
        val repo = DetailFakeRepository()
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.choose("Approve")
        val old = vm.state.value.confirmation!!
        repo.current.value = detailItem.copy(version = 2, choices = listOf("Other"))
        runCurrent(); vm.confirm(old)
        assertTrue(repo.answers.isEmpty())
        vm.choose("Other")
        val newer = vm.state.value.confirmation!!
        repo.syncState.value = SyncState.Stale(detailNow, Exception("offline"))
        runCurrent()
        assertEquals("Choose rollout", vm.state.value.item?.title)
        assertFalse(vm.state.value.actionsEnabled)
        assertNull(vm.state.value.confirmation)
        repo.syncState.value = SyncState.Current(detailNow)
        runCurrent(); vm.confirm(newer); runCurrent()
        assertTrue(repo.answers.isEmpty())
    }

    @Test fun ambiguousFailureShowsRepositoryReconciledOutcomeWithoutResending() = runTest {
        val repo = DetailFakeRepository().apply { answerSucceeds = false }
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.choose("Approve"); vm.confirm(vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(StatusDto.RESOLVED, vm.state.value.item?.status)
        assertEquals(1, repo.answers.size)
        assertFalse(vm.state.value.actionsEnabled)
        assertNull(vm.state.value.confirmation)
    }

    @Test fun failedReplyLeavesNoAutomaticRetryAndAllowsFreshChoiceAfterReconciliation() = runTest {
        val repo = DetailFakeRepository().apply { answerSucceeds = false; closeOnAnswer = false }
        val vm = DecisionViewModel("request", repo)
        runCurrent(); vm.choose("Reject"); vm.confirm(vm.state.value.confirmation!!)
        runCurrent()
        assertTrue(vm.state.value.answerFailed)
        assertNull(vm.state.value.confirmation)
        assertEquals(1, repo.answers.size)
        vm.choose("Approve")
        assertNotNull(vm.state.value.confirmation)
        assertEquals(1, repo.answers.size)
    }

    @Test fun coldLoadFailureIsRetryableAndNeverShownAsMissing() = runTest {
        val repo = DetailFakeRepository().apply { current.value = null; refreshSucceeds = false; refreshGate = CompletableDeferred() }
        val vm = DecisionViewModel("request", repo)
        runCurrent()
        assertTrue(vm.state.value.loading)
        repo.refreshGate!!.complete(Unit); runCurrent()
        assertTrue(vm.state.value.loadFailed)
        assertFalse(vm.state.value.missing)
        repo.refreshSucceeds = true; repo.current.value = detailItem
        vm.refresh(); runCurrent()
        assertEquals(detailItem, vm.state.value.item)
        assertFalse(vm.state.value.loadFailed)
    }

    @Test fun confirmedAbsentItemIsMissingAndCachedItemSurvivesReadFailure() = runTest {
        val absent = DetailFakeRepository().apply { current.value = null }
        val missingVm = DecisionViewModel("request", absent)
        runCurrent(); assertTrue(missingVm.state.value.missing)
        val cached = DetailFakeRepository().apply { refreshSucceeds = false }
        val cachedVm = DecisionViewModel("request", cached)
        runCurrent()
        assertEquals(detailItem, cachedVm.state.value.item)
        assertTrue(cachedVm.state.value.loadFailed)
        assertFalse(cachedVm.state.value.actionsEnabled)
    }
}

internal val detailNow: Instant = Instant.parse("2026-09-04T12:00:00Z")
internal val detailItem = Item("request", "Release agent", "Choose rollout", "## Context\n\nReady to deploy.", "host", "lam",
    PriorityDto.NORMAL, listOf("Approve", "Reject"), emptyList(), "Start gradually.", "Approve", "", StatusDto.OPEN,
    null, null, null, "2026-09-04T11:55:00Z", null, null, 1)

internal class DetailFakeRepository : ItemRepository {
    val current = MutableStateFlow<Item?>(detailItem)
    override val openItems = current.map { listOfNotNull(it) }
    override val syncState = MutableStateFlow<SyncState>(SyncState.Current(detailNow))
    override val errors = emptyFlow<Exception>()
    val answers = mutableListOf<FinalAnswer>()
    var answerGate: CompletableDeferred<Unit>? = null
    var refreshGate: CompletableDeferred<Unit>? = null
    var refreshSucceeds = true
    var answerSucceeds = true
    var closeOnAnswer = true
    override fun item(id: String) = current
    override suspend fun refreshItem(id: String): Boolean {
        refreshGate?.await()
        if (!refreshSucceeds) syncState.value = SyncState.Stale(detailNow, Exception("offline"))
        return refreshSucceeds
    }
    override suspend fun answer(id: String, answer: FinalAnswer): Boolean {
        answers += answer
        answerGate?.await()
        if (closeOnAnswer) current.value = current.value!!.copy(status = StatusDto.RESOLVED,
            responseChoice = (answer as? FinalAnswer.Choice)?.value, responseText = (answer as? FinalAnswer.Text)?.value,
            responseBy = ResponseByDto.PHONE, version = 2)
        return answerSucceeds
    }
    override suspend fun refresh(): Boolean { syncState.value = SyncState.Current(detailNow); return true }
    override fun history(query: HistoryQuery) = flowOf(emptyList<Item>())
    override val cachedHistory = flowOf(emptyList<Item>())
    override suspend fun refreshHistory(query: HistoryQuery, cursor: String?) = HistoryResult(false, null)
    override suspend fun setCheck(id: String, index: Int, done: Boolean) = false
    override suspend fun unpair() = Unit
}
