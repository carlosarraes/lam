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
) : AutoCloseable {
    private val owner = SupervisorJob(scope.coroutineContext[Job])
    private val work = CoroutineScope(scope.coroutineContext + owner)
    private val foreground = MutableStateFlow(lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED))
    private val observer = LifecycleEventObserver { _, _ ->
        foreground.value = lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    }
    private val lock = Mutex()
    private data class Refresh(val session: Long, val result: Deferred<Boolean>)
    private var inFlight: Refresh? = null

    init {
        lifecycle.addObserver(observer)
        work.launch {
            var seenForeground = false
            var observedSession: Long? = null
            combine(sessions, foreground) { session, active -> session to active }.distinctUntilChanged().collect { (session, active) ->
                // A direct post-save refresh can precede delivery of a queued old session event.
                lock.withLock {
                    if (session != sessions.value) return@collect
                    if (inFlight?.session != session) {
                        inFlight?.result?.cancel()
                        inFlight = null
                    }
                }
                if (observedSession != session) seenForeground = false
                observedSession = session
                if (session != null && active) {
                    val alreadyRefreshed = !seenForeground && repository.syncState.value is SyncState.Current
                    seenForeground = true
                    if (!alreadyRefreshed) work.launch { refresh(session) }
                }
            }
        }
    }

    suspend fun refresh(): Boolean = sessions.value?.let { refresh(it) } ?: false

    private suspend fun refresh(session: Long): Boolean {
        val operation = lock.withLock {
            if (session != sessions.value) return false
            val current = inFlight
            if (current?.session == session && !current.result.isCompleted) current.result else {
                current?.result?.cancel()
                work.async {
                    if (session == sessions.value) repository.refresh() else false
                }.also { inFlight = Refresh(session, it) }
            }
        }
        return operation.await()
    }

    override fun close() {
        lifecycle.removeObserver(observer)
        owner.cancel()
    }
}
