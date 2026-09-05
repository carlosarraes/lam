package dev.carraes.lam.sync

import androidx.lifecycle.*
import dev.carraes.lam.items.SyncState
import dev.carraes.lam.ui.requests.RequestsFakeRepository
import dev.carraes.lam.ui.requests.now
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class LifecycleReconcilerTest {
    private class Owner : LifecycleOwner {
        override val lifecycle = LifecycleRegistry.createUnsafe(this)
        fun foreground() { lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_START) }
        fun background() { lifecycle.handleLifecycleEvent(Lifecycle.Event.ON_STOP) }
    }

    @Test fun restoredPairingRefreshesOnLaunchAndEveryResumeIncludingAfterFailure() = runTest {
        val owner = Owner(); val repo = RequestsFakeRepository(); val paired = MutableStateFlow(true)
        val reconciler = LifecycleReconciler(repo, paired, owner.lifecycle, backgroundScope)
        runCurrent(); assertEquals(0, repo.refreshes)
        owner.foreground(); runCurrent(); assertEquals(1, repo.refreshes)
        owner.background(); runCurrent(); repo.succeeds = false
        owner.foreground(); runCurrent(); assertEquals(2, repo.refreshes)
        assertTrue(repo.syncState.value is SyncState.Stale)
        owner.background(); runCurrent(); repo.succeeds = true
        owner.foreground(); runCurrent(); assertEquals(3, repo.refreshes)
        assertTrue(repo.syncState.value is SyncState.Current)
        reconciler.close()
        owner.background(); owner.foreground(); runCurrent(); assertEquals(3, repo.refreshes)
    }

    @Test fun pairingAndSimultaneousManualAndForegroundTriggersShareOneRefresh() = runTest {
        val owner = Owner(); val repo = RequestsFakeRepository(); val paired = MutableStateFlow(false)
        repo.gate = CompletableDeferred()
        val reconciler = LifecycleReconciler(repo, paired, owner.lifecycle, backgroundScope)
        owner.foreground(); runCurrent(); assertEquals(0, repo.refreshes)
        paired.value = true
        val one = async { reconciler.refresh() }; val two = async { reconciler.refresh() }
        runCurrent(); assertEquals(1, repo.refreshes)
        owner.background(); runCurrent(); owner.foreground(); runCurrent()
        assertEquals(1, repo.refreshes)
        repo.gate!!.complete(Unit); runCurrent()
        assertTrue(one.await()); assertTrue(two.await())
        advanceTimeBy(600_000); runCurrent(); assertEquals(1, repo.refreshes)
        reconciler.close()
    }

    @Test fun completedPairingRefreshIsNotRepeatedOnFirstForeground() = runTest {
        val owner = Owner(); val repo = RequestsFakeRepository()
        repo.syncState.value = SyncState.Current(now)
        val reconciler = LifecycleReconciler(repo, MutableStateFlow(true), owner.lifecycle, backgroundScope)
        owner.foreground(); runCurrent(); assertEquals(0, repo.refreshes)
        owner.background(); runCurrent(); owner.foreground(); runCurrent(); assertEquals(1, repo.refreshes)
        reconciler.close()
    }
}
