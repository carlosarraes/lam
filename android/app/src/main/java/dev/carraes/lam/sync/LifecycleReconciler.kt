package dev.carraes.lam.sync

import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import dev.carraes.lam.items.ItemRepository
import dev.carraes.lam.items.SyncState
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

class LifecycleReconciler(
    private val repository: ItemRepository,
    private val sessions: StateFlow<Long?>,
    private val lifecycle: Lifecycle,
    scope: CoroutineScope,
    private val connectivity: StateFlow<ConnectivityStatus> = MutableStateFlow(ConnectivityStatus(true, 0)),
) : AutoCloseable {
    private val owner = SupervisorJob(scope.coroutineContext[Job])
    private val work = CoroutineScope(scope.coroutineContext + owner)
    private val foreground = MutableStateFlow(lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED))
    private val observer = LifecycleEventObserver { _, _ ->
        foreground.value = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    }
    private val lock = Mutex()
    private data class Refresh(val session: Long, val epoch: Long, val result: Deferred<Boolean>)
    private var inFlight: Refresh? = null
    private val completed = MutableStateFlow(0L)
    val completedReconciliations = completed.asStateFlow()

    init {
        lifecycle.addObserver(observer)
        work.launch {
            var seenForeground = false
            var observedSession: Long? = null
            combine(sessions, foreground, connectivity) { session, active, network -> Triple(session, active, network) }
                .distinctUntilChanged().collect { (session, active, network) ->
                // A direct post-save refresh can precede delivery of a queued old session event.
                lock.withLock {
                    if (session != sessions.value || network != connectivity.value) return@collect
                    if (inFlight?.session != session || inFlight?.epoch != network.epoch) {
                        inFlight?.result?.cancel()
                        inFlight = null
                    }
                }
                if (observedSession != session) seenForeground = false
                observedSession = session
                if (session != null && active && network.available) {
                    val firstForeground = !seenForeground
                    seenForeground = true
                    work.launch(start = CoroutineStart.UNDISPATCHED) {
                        // A post-pairing read can finish after this trigger was queued.
                        if (!firstForeground || repository.syncState.value !is SyncState.Current) refresh(session)
                    }
                }
            }
        }
    }

    suspend fun refresh(): Boolean = sessions.value?.let { refresh(it) } ?: false

    private suspend fun refresh(session: Long): Boolean {
        val operation = lock.withLock {
            if (session != sessions.value) return false
            val network = connectivity.value
            if (!network.available) return false
            val current = inFlight
            if (current?.session == session && current.epoch == network.epoch && !current.result.isCompleted) current.result else {
                current?.result?.cancel()
                work.async {
                    val succeeded = session == sessions.value && repository.refresh()
                    if (succeeded && session == sessions.value && connectivity.value == network) completed.value++
                    succeeded
                }.also { inFlight = Refresh(session, network.epoch, it) }
            }
        }
        return operation.await()
    }

    override fun close() {
        lifecycle.removeObserver(observer)
        owner.cancel()
    }
}
