package dev.carraes.lam.items

import dev.carraes.lam.ui.detail.DecisionViewModel
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
class DecisionRepositoryIntegrationTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun committedReplyWithLostResponseKeepsCanonicalOutcomeOnVisibleDetail() = runTest {
        assertCommittedReply(ApiError.Transport("response lost"))
    }

    @Test fun serverErrorAfterCommitAlsoFetchesCanonicalOutcomeWithoutReplayingAnswer() = runTest {
        assertCommittedReply(ApiError.Server(502, null))
    }

    private suspend fun TestScope.assertCommittedReply(error: ApiError) {
        val f = fixture(error)
        f.vm.choose("done")
        f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        assertEquals("detail must retain the canonical closed item", StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals("done", f.vm.state.value.item?.responseChoice)
        assertEquals(ResponseByDto.PHONE, f.vm.state.value.item?.responseBy)
        assertEquals("2026-09-04T12:00:00Z", f.vm.state.value.item?.resolvedAt)
        assertFalse(f.vm.state.value.missing)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertTrue(f.repo.openItems.first().isEmpty())
        assertEquals(listOf("get:a", "choice:a:done", "get:a", "open"), f.api.calls)
        assertEquals(1, f.api.submissions)
        assertEquals(SyncState.Current(ItemRepositoryTest.NOW), f.repo.syncState.value)
    }

    @Test fun mutationsStayDisabledThroughCanonicalReadAndFullQueueReconciliation() = runTest {
        val f = fixture()
        val targetGate = CompletableDeferred<Unit>()
        val queueGate = CompletableDeferred<Unit>()
        f.api.beforeGet = { if (f.api.submissions > 0) targetGate.await() }
        f.api.beforeRead = { queueGate.await() }
        f.vm.choose("done"); f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(StatusDto.OPEN, f.vm.state.value.item?.status)
        assertTrue(f.vm.state.value.submitting)
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        targetGate.complete(Unit); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        assertTrue(f.vm.state.value.submitting)
        queueGate.complete(Unit); runCurrent()
        assertEquals(SyncState.Current(ItemRepositoryTest.NOW), f.repo.syncState.value)
        assertFalse(f.vm.state.value.submitting)
        assertEquals(1, f.api.submissions)
    }

    @Test fun failedCanonicalReadKeepsLastSnapshotAndLaterRefreshReadsBeforePruning() = runTest {
        val f = fixture()
        f.api.beforeGet = { if (f.api.submissions > 0) throw ApiError.Transport("read offline") }
        f.vm.choose("done"); f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(StatusDto.OPEN, f.vm.state.value.item?.status)
        assertEquals("Body", f.vm.state.value.item?.body)
        assertFalse(f.vm.state.value.missing)
        assertTrue(f.vm.state.value.answerFailed)
        assertTrue(f.repo.syncState.value is SyncState.Stale)
        assertFalse(f.vm.state.value.actionsEnabled)
        assertEquals(listOf("get:a", "choice:a:done", "get:a"), f.api.calls)
        f.api.beforeGet = {}
        assertTrue(f.repo.refresh()); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals(ResponseByDto.PHONE, f.vm.state.value.item?.responseBy)
        assertEquals(listOf("get:a", "choice:a:done", "get:a", "get:a", "open"), f.api.calls)
        assertEquals(1, f.api.submissions)
    }

    @Test fun failedFullReconciliationRetainsClosedOutcomeAndBlocksMutationsUntilRetry() = runTest {
        val f = fixture()
        f.api.beforeRead = { throw ApiError.Transport("queue offline") }
        f.vm.choose("done"); f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertFalse(f.repo.syncState.value.mutationsEnabled)
        assertFalse(f.vm.state.value.submitting)
        f.api.beforeRead = {}
        assertTrue(f.repo.refresh()); runCurrent()
        assertEquals(SyncState.Current(ItemRepositoryTest.NOW), f.repo.syncState.value)
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals(1, f.api.submissions)
    }

    @Test fun unauthorizedCanonicalReadRevokesSessionWithoutRestoringOldCache() = runTest {
        val f = fixture()
        f.api.beforeGet = { if (f.api.submissions > 0) throw ApiError.Unauthorized(null) }
        f.vm.choose("done"); f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        assertEquals(SyncState.Revoked, f.repo.syncState.value)
        assertNull(f.credentials.state.value)
        assertNull(f.repo.item("a").first())
        assertFalse(f.vm.state.value.actionsEnabled)
        assertEquals(listOf("get:a", "choice:a:done", "get:a"), f.api.calls)
        assertEquals(1, f.api.submissions)
    }

    @Test fun credentialReplacementDiscardsPendingRecoveryAndRejectsItsLateResult() = runTest {
        val f = fixture()
        val targetGate = CompletableDeferred<Unit>()
        f.api.beforeGet = { if (f.api.submissions > 0) targetGate.await() }
        f.vm.choose("done"); f.vm.confirm(f.vm.state.value.confirmation!!)
        runCurrent()
        val server = f.credentials.state.value!!
        f.repo.credentialStore.save(server, "synthetic-replacement")
        targetGate.complete(Unit); runCurrent()
        assertEquals(SyncState.Idle, f.repo.syncState.value)
        assertNull(f.repo.item("a").first())
        assertEquals(server, f.credentials.state.value)
        f.api.calls.clear()
        f.api.beforeGet = { error("the replacement must not inherit pending reads") }
        assertTrue(f.repo.refresh())
        assertEquals(listOf("open"), f.api.calls)
        assertNull(f.repo.item("a").first())
    }

    @Test fun cancellationDuringRecoveryPropagatesAndKeepsReadPendingForNextRefresh() = runTest {
        val f = fixture()
        f.api.beforeGet = { if (f.api.submissions > 0) CompletableDeferred<Unit>().await() }
        val answer = launch { f.repo.answer("a", FinalAnswer.Choice("done")) }
        runCurrent()
        answer.cancelAndJoin(); runCurrent()
        assertTrue(answer.isCancelled)
        assertTrue(f.repo.syncState.value is SyncState.Stale)
        assertEquals(StatusDto.OPEN, f.vm.state.value.item?.status)
        assertFalse(f.vm.state.value.actionsEnabled)
        f.api.beforeGet = {}
        assertTrue(f.repo.refresh()); runCurrent()
        assertEquals(StatusDto.RESOLVED, f.vm.state.value.item?.status)
        assertEquals(1, f.api.submissions)
    }

    private suspend fun TestScope.fixture(error: ApiError = ApiError.Transport("response lost")): Fixture {
        val api = FakeApi().apply { fetched = ItemRepositoryTest.item().copy(checks = emptyList()) }
        val credentials = FakeCredentials()
        val repo = DefaultItemRepository(MemoryStorage(), { api }, credentials, backgroundScope,
            Clock.fixed(ItemRepositoryTest.NOW, ZoneOffset.UTC))
        assertTrue(repo.refresh())
        val vm = DecisionViewModel("a", repo)
        runCurrent()
        assertTrue(vm.state.value.actionsEnabled)
        api.calls.clear()
        api.writeError = error
        // The server commits, then the transport loses the response. Only getItem can recover the outcome.
        api.beforeWrite = {
            api.fetched = api.fetched.copy(status = StatusDto.RESOLVED, version = 2, responseChoice = "done",
                responseBy = ResponseByDto.PHONE, resolvedAt = "2026-09-04T12:00:00Z")
            api.open = emptyList()
        }
        return Fixture(api, credentials, repo, vm)
    }

    private data class Fixture(val api: FakeApi, val credentials: FakeCredentials, val repo: DefaultItemRepository, val vm: DecisionViewModel)
}
