package dev.carraes.lam.items

import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import java.time.Clock
import java.time.Instant
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

internal class DefaultItemRepository(
    private val storage: ItemStorage,
    private val apiProvider: () -> LamApi?,
    private val credentials: CredentialStore,
    scope: CoroutineScope,
    private val clock: Clock = Clock.systemUTC(),
) : ItemRepository {
    // Network operations are serialized. Credential changes can still invalidate an in-flight call.
    private val operations = Mutex()
    private val stateLock = Mutex()
    private val initialized = CompletableDeferred<Unit>()
    private var generation = 0L
    private var paired: PairedServer? = null
    private var lastSuccess: Instant? = null
    private val state = MutableStateFlow<SyncState>(SyncState.Idle)
    private val errorEvents = Channel<Exception>(Channel.BUFFERED)

    override val openItems = storage.openItems().map { it.map(ItemMapper::toItem) }
    override val syncState = state.asStateFlow()
    override val errors = errorEvents.receiveAsFlow()
    override fun item(id: String) = storage.item(id).map { it?.let(ItemMapper::toItem) }
    override fun history(query: HistoryQuery) = storage.history(query.key).map { it.map(ItemMapper::toItem) }

    init {
        scope.launch {
            credentials.observe().collect { server ->
                stateLock.withLock {
                    generation++
                    val changedAccount = paired?.let { it.serverUrl != server?.serverUrl || it.deviceId != server.deviceId } == true
                    if (server == null || changedAccount) {
                        storage.clear()
                        lastSuccess = null
                    } else if (!initialized.isCompleted) {
                        lastSuccess = storage.lastSuccess()
                    }
                    paired = server
                    if (state.value != SyncState.Revoked || server != null) state.value = SyncState.Idle
                    initialized.complete(Unit)
                }
            }
        }
    }

    override suspend fun refresh(): Boolean = operation { session -> reconcile(session) }

    override suspend fun refreshItem(id: String): Boolean = operation { session ->
        val item = session.api.getItem(id)
        commit(session) { storage.upsert(listOf(ItemMapper.toEntity(item))) }
    }

    override suspend fun refreshHistory(query: HistoryQuery, cursor: String?): HistoryResult {
        var nextCursor: String? = null
        val succeeded = operation { session ->
            val page = session.api.getHistory(query.query, query.priority, query.type, cursor)
            commit(session) {
                storage.cacheHistory(query.key, page.items.map(ItemMapper::toEntity), replace = cursor == null)
                nextCursor = page.nextCursor
            }
        }
        return HistoryResult(succeeded, nextCursor)
    }

    override suspend fun answer(id: String, answer: FinalAnswer): Boolean = operation(mutation = true) { session ->
        val canonical = session.api.getItem(id)
        var open = false
        if (!commit(session) {
            storage.upsert(listOf(ItemMapper.toEntity(canonical)))
            open = storage.get(id)?.canonical?.status == StatusDto.OPEN
        }) return@operation false
        if (!open) {
            errorEvents.trySend(ApiError.AlreadyClosed(null))
            return@operation false
        }
        // The generation guard prevents submission after unpair while the preflight read was pending.
        if (!isCurrent(session)) return@operation false
        try {
            val result = when (answer) {
                is FinalAnswer.Choice -> session.api.replyChoice(id, answer.value)
                is FinalAnswer.Text -> session.api.replyText(id, answer.value)
                FinalAnswer.Dismiss -> session.api.dismiss(id)
            }
            commit(session) { storage.upsert(listOf(ItemMapper.toEntity(result))) }
        } catch (error: ApiError) {
            mutationFailed(session, id, error)
            false
        }
    }

    override suspend fun setCheck(id: String, index: Int, done: Boolean): Boolean = operation(mutation = true) { session ->
        val snapshot = stateLock.withLock {
            if (session.generation != generation) null else storage.get(id)
        } ?: return@operation false
        if (snapshot.canonical.status != StatusDto.OPEN || index !in snapshot.canonical.checks.indices) return@operation false
        val tag = UUID.randomUUID().toString()
        val checks = snapshot.canonical.checks.mapIndexed { i, check ->
            if (i == index) check.copy(done = done, at = if (done) clock.instant().toString() else null) else check
        }
        if (!commit(session) { storage.optimistic(snapshot, snapshot.copy(canonical = snapshot.canonical.copy(checks = checks), optimisticTag = tag)) }) return@operation false
        try {
            val canonical = session.api.setCheck(id, index, done)
            commit(session) { storage.upsert(listOf(ItemMapper.toEntity(canonical))) }
        } catch (error: Exception) {
            withContext(NonCancellable) {
                commit(session) { storage.rollback(snapshot, tag) }
            }
            if (error is CancellationException) throw error
            mutationFailed(session, id, error)
            false
        }
    }

    override suspend fun unpair() = withContext(NonCancellable) {
        clearSession(revoked = false)
        credentials.clear()
    }

    private suspend fun reconcile(session: Session): Boolean {
        if (!commit(session) { state.value = SyncState.Refreshing }) return false
        val items = session.api.listOpenItems().map(ItemMapper::toEntity)
        return commit(session) {
            val now = clock.instant()
            storage.reconcile(items, now)
            lastSuccess = now
            state.value = SyncState.Current(now)
        }
    }

    private suspend fun mutationFailed(session: Session, id: String, error: Exception) {
        handleFailure(session, error)
        if (error is ApiError.Transport || error is ApiError.AlreadyClosed || error is ApiError.Server) {
            // A write may have committed remotely. Never replay it; establish canonical state first.
            if (!isCurrent(session)) return
            try {
                if (error is ApiError.AlreadyClosed) {
                    val canonical = session.api.getItem(id)
                    commit(session) { storage.upsert(listOf(ItemMapper.toEntity(canonical))) }
                }
                reconcile(session)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (refreshError: Exception) {
                handleFailure(session, refreshError, emit = false)
            }
        }
    }

    private suspend fun operation(mutation: Boolean = false, block: suspend (Session) -> Boolean): Boolean {
        initialized.await()
        return operations.withLock {
            val session = stateLock.withLock {
                if (paired == null || (mutation && !state.value.mutationsEnabled)) null
                else apiProvider()?.let { Session(generation, it) }
            } ?: return@withLock false
            try {
                block(session)
            } catch (cancelled: CancellationException) {
                withContext(NonCancellable) { handleFailure(session, cancelled, emit = false) }
                throw cancelled
            } catch (error: Exception) {
                handleFailure(session, error)
                false
            }
        }
    }

    private suspend fun handleFailure(session: Session, error: Exception, emit: Boolean = true) {
        if (error is ApiError.Unauthorized) {
            withContext(NonCancellable) {
                val cleared = stateLock.withLock {
                    if (session.generation != generation) false else {
                        clearSessionLocked(revoked = true)
                        true
                    }
                }
                if (cleared) credentials.clear()
            }
        } else {
            commit(session) { state.value = SyncState.Stale(lastSuccess, error) }
        }
        if (emit && isCurrent(session)) errorEvents.trySend(error)
    }

    private suspend fun clearSession(revoked: Boolean) = stateLock.withLock { clearSessionLocked(revoked) }

    private suspend fun clearSessionLocked(revoked: Boolean) {
        generation++
        paired = null
        storage.clear()
        lastSuccess = null
        state.value = if (revoked) SyncState.Revoked else SyncState.Idle
    }

    private suspend fun isCurrent(session: Session): Boolean = stateLock.withLock { session.generation == generation }

    private suspend fun commit(session: Session, write: suspend () -> Unit): Boolean = stateLock.withLock {
        if (session.generation != generation) false else {
            write()
            true
        }
    }

    private data class Session(val generation: Long, val api: LamApi)
}
