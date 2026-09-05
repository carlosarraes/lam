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
    paired: Flow<Boolean>,
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
    private var inFlight: Deferred<Boolean>? = null

    init {
        lifecycle.addObserver(observer)
        work.launch {
            var seenForeground = false
            combine(paired, foreground) { ready, active -> ready to active }.distinctUntilChanged().collect { (ready, active) ->
                if (!ready) seenForeground = false
                if (ready && active) {
                    val alreadyRefreshed = !seenForeground && repository.syncState.value is SyncState.Current
                    seenForeground = true
                    if (!alreadyRefreshed) work.launch { refresh() }
                }
            }
        }
    }

    suspend fun refresh(): Boolean {
        val operation = lock.withLock {
            inFlight?.takeUnless { it.isCompleted } ?: work.async { repository.refresh() }.also { inFlight = it }
        }
        return operation.await()
    }

    override fun close() {
        lifecycle.removeObserver(observer)
        owner.cancel()
    }
}
