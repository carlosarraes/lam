package dev.carraes.lam.items

import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import dev.carraes.lam.diagnostics.*
import dev.carraes.lam.sync.ConnectivityStatus
import kotlinx.coroutines.flow.StateFlow
import java.time.Clock
import java.time.Instant
import java.util.UUID
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.emitAll
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
    private val connectivity: StateFlow<ConnectivityStatus> = MutableStateFlow(ConnectivityStatus(true, 0)),
) : ItemRepository, DeviceSettings {
    override suspend fun diagnosticData(): DiagnosticData {
        initialized.await()
        return stateLock.withLock { DiagnosticData(paired?.serverUrl, lastSuccess, storage.counts(), recentErrors.toList()) }
    }
    override val device get() = credentialStore.observe()
    override suspend fun revoke(expectedSession: Long): UnpairResult {
        initialized.await()
        return operations.withLock {
            val current = stateLock.withLock {
                if (generation != expectedSession || paired == null) null
                else apiProvider()?.let { Session(generation, it, connectivity.value.epoch) }
            } ?: return@withLock UnpairResult.SESSION_CHANGED
            try {
                current.api.revokeDevice()
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: Exception) {
                if (error !is ApiError.Unauthorized) {
                    handleFailure(current, error)
                    return@withLock if (isCurrent(current)) UnpairResult.UNAVAILABLE
                        else UnpairResult.SESSION_CHANGED
                }
                // A rejected credential is already unusable remotely; local cleanup is sufficient.
            }
            if (eraseLocal(expectedSession)) UnpairResult.REVOKED
            else UnpairResult.SESSION_CHANGED
        }
    }

    override suspend fun eraseLocal(expectedSession: Long): Boolean = withContext(NonCancellable) {
        credentialChanges.withLock {
            val current = stateLock.withLock {
                if (generation != expectedSession || paired == null) null
                else apiProvider()?.let { Session(generation, it, connectivity.value.epoch) }
            } ?: return@withLock false
            clearSession(revoked = false, expected = current)
            true
        }
    }
    // Network operations are serialized. Credential changes can still invalidate an in-flight call.
    private val operations = Mutex()
    private val stateLock = Mutex()
    private val credentialChanges = Mutex()
    private val initialized = CompletableDeferred<Unit>()
    private var generation = 0L
    // Opaque process-local identity, published only after canonical session setup completes.
    private val session = MutableStateFlow<Long?>(null)
    override val reconciliationSession = session.asStateFlow()
    private var paired: PairedServer? = null
    private var lastSuccess: Instant? = null
    private var reconciledConnectionEpoch: Long? = null
    private val recentErrors = ArrayDeque<ErrorCategory>()
    // One read recovery at most: operations serialize, and stale state blocks the next final answer.
    private var unresolvedFinalAnswer: String? = null
    private val observedItems = mutableMapOf<String, Int>()
    private val state = MutableStateFlow<SyncState>(SyncState.Idle)
    private val errorEvents = Channel<Exception>(Channel.BUFFERED)
    private val pairing = MutableSharedFlow<PairedServer?>(replay = 1, onBufferOverflow = BufferOverflow.DROP_OLDEST)

    override val openItems = storage.openItems().map { it.map(ItemMapper::toItem) }
    override val cachedHistory = storage.cachedHistory().map { it.map(ItemMapper::toItem) }
    override val syncState = state.asStateFlow()
    override val errors = errorEvents.receiveAsFlow()
    val credentialStore: CredentialStore = object : CredentialStore {
        override fun observe() = pairing.asSharedFlow()
        override suspend fun save(server: PairedServer, credential: String) = replaceCredential(server, credential)
        override suspend fun clear() = unpair()
    }
    override fun item(id: String) = flow {
        initialized.await()
        val observedGeneration = stateLock.withLock {
            observedItems[id] = (observedItems[id] ?: 0) + 1
            generation
        }
        try {
            emitAll(storage.item(id).map { it?.let(ItemMapper::toItem) })
        } finally {
            withContext(NonCancellable) {
                stateLock.withLock {
                    if (generation == observedGeneration) {
                        val remaining = (observedItems[id] ?: 1) - 1
                        if (remaining == 0) observedItems.remove(id) else observedItems[id] = remaining
                    }
                }
            }
        }
    }
    override fun history(query: HistoryQuery) = storage.history(query.key).map { it.map(ItemMapper::toItem) }

    init {
        scope.launch {
            initialized.await()
            connectivity.collect { network ->
                stateLock.withLock {
                    val previousConnection = state.value is SyncState.Current && reconciledConnectionEpoch != network.epoch
                    if ((!network.available || previousConnection) && network == connectivity.value && paired != null) {
                        state.value = SyncState.Stale(lastSuccess, ApiError.Transport("network_unavailable"))
                    }
                }
            }
        }
        scope.launch {
            // Drain the suspending publisher independently of the lock held by save/clear.
            credentials.observe().conflate().collect {
                credentialChanges.withLock {
                    // Queued transitions can supersede the wake-up event. Read replayed current metadata.
                    val server = credentials.observe().first()
                    val failures = mutableListOf<Exception>()
                    stateLock.withLock state@ {
                        val restoring = !initialized.isCompleted
                        // Managed saves already published this session, including same-metadata replacements.
                        if (!restoring && server == paired) return@state
                        invalidateSessionLocked(revoked = server == null && state.value == SyncState.Revoked)
                        try {
                            if (restoring && server != null) lastSuccess = storage.lastSuccess()
                            else storage.clear()
                            paired = server
                        } catch (cancelled: CancellationException) {
                            throw cancelled
                        } catch (error: Exception) {
                            failures += error
                        } finally {
                            session.value = paired?.let { generation }
                            pairing.tryEmit(paired)
                            initialized.complete(Unit)
                        }
                    }
                    if (failures.isNotEmpty()) {
                        attemptCredentialClear(failures)
                        errorEvents.trySend(SessionCleanupException(failures))
                    }
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
        if (canonical.kind == ItemKindDto.FYI && answer != FinalAnswer.Dismiss) return@operation false
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
        if (!prepareSubmission(session, id)) return@operation false
        try {
            val result = when (answer) {
                FinalAnswer.Complete -> session.api.complete(id)
                is FinalAnswer.Choice -> session.api.replyChoice(id, answer.value)
                is FinalAnswer.Text -> session.api.replyText(id, answer.value)
                FinalAnswer.Dismiss -> session.api.dismiss(id)
            }
            commit(session) {
                storage.upsert(listOf(ItemMapper.toEntity(result)))
                unresolvedFinalAnswer = null
            }
        } catch (error: ApiError) {
            mutationFailed(session, id, error, finalAnswer = true)
            false
        }
    }

    override suspend fun setCheck(id: String, index: Int, done: Boolean): Boolean = operation(mutation = true) { session ->
        val snapshot = stateLock.withLock {
            if (session.generation != generation) null else storage.get(id)
        } ?: return@operation false
        if (snapshot.canonical.kind == ItemKindDto.FYI || snapshot.canonical.status != StatusDto.OPEN || index !in snapshot.canonical.checks.indices) return@operation false
        val tag = UUID.randomUUID().toString()
        val checks = snapshot.canonical.checks.mapIndexed { i, check ->
            if (i == index) check.copy(done = done, at = if (done) clock.instant().toString() else null) else check
        }
        if (!commit(session) { storage.optimistic(snapshot, snapshot.copy(canonical = snapshot.canonical.copy(checks = checks), optimisticTag = tag)) }) return@operation false
        if (!prepareSubmission(session, id)) {
            withContext(NonCancellable) { commit(session) { storage.rollback(snapshot, tag) } }
            return@operation false
        }
        try {
            val canonical = session.api.setCheck(id, index, done)
            commit(session) {
                storage.upsert(listOf(ItemMapper.toEntity(canonical)))
                unresolvedFinalAnswer = null
            }
        } catch (error: Exception) {
            withContext(NonCancellable) {
                commit(session) { storage.rollback(snapshot, tag) }
            }
            if (error is CancellationException) throw error
            // A final check can close the request; recover its outcome before the open queue prunes it.
            mutationFailed(session, id, error, finalAnswer = true)
            false
        }
    }

    override suspend fun unpair() = withContext(NonCancellable) {
        credentialChanges.withLock {
            try {
                clearSession(revoked = false)
            } catch (error: SessionCleanupException) {
                errorEvents.trySend(error)
                throw error
            }
        }
    }

    override suspend fun markSeen(id: String, version: Long): Boolean = operation(mutation = true) { session ->
        val snapshot = stateLock.withLock {
            if (session.generation != generation) null else storage.get(id)?.canonical
        } ?: return@operation false
        if (snapshot.kind != ItemKindDto.FYI) return@operation false
        if (snapshot.seenAt != null) return@operation true
        if (snapshot.status != StatusDto.OPEN || !prepareSubmission(session, id)) return@operation false
        try {
            val canonical = session.api.markSeen(id, version)
            val committed = commit(session) {
                storage.upsert(listOf(ItemMapper.toEntity(canonical)))
                unresolvedFinalAnswer = null
            }
            committed && canonical.kind == ItemKindDto.FYI && canonical.seenAt != null
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            mutationFailed(session, id, error, finalAnswer = true)
            false
        }
    }

    private suspend fun replaceCredential(server: PairedServer, credential: String) {
        require(credential.isNotBlank()) { "credential must not be blank" }
        initialized.await()
        withContext(NonCancellable) {
            credentialChanges.withLock {
                stateLock.withLock { invalidateSessionLocked(revoked = false) }
                try {
                    stateLock.withLock { storage.clear() }
                    credentials.save(server, credential)
                    stateLock.withLock {
                        paired = server
                        session.value = generation
                        pairing.tryEmit(server)
                    }
                } catch (error: Exception) {
                    pairing.tryEmit(null)
                    val failures = mutableListOf(error)
                    attemptCredentialClear(failures)
                    val failure = SessionCleanupException(failures)
                    errorEvents.trySend(failure)
                    throw failure
                }
            }
        }
    }

    private suspend fun reconcile(session: Session): Boolean {
        var unresolved: String? = null
        if (!connectionIsCurrent(session)) return false
        if (!commit(session) {
            state.value = SyncState.Refreshing
            unresolved = unresolvedFinalAnswer
        }) return false
        // An absent open row says nothing about its answer. Recover that outcome before pruning it.
        // If either read fails, retain the snapshot and pending ID for the next explicit reconciliation.
        unresolved?.let { id ->
            val canonical = session.api.getItem(id)
            if (!commit(session) { storage.upsert(listOf(ItemMapper.toEntity(canonical))) }) return false
        }
        val items = session.api.listOpenItems().map(ItemMapper::toEntity)
        val observedMissing = stateLock.withLock {
            if (session.generation != generation) return false
            observedItems.keys.filter { id ->
                id != unresolved && items.none { it.canonical.id == id } &&
                    storage.get(id)?.canonical?.status == StatusDto.OPEN
            }
        }
        for (id in observedMissing) {
            val canonical = session.api.getItem(id)
            if (!commit(session) { storage.upsert(listOf(ItemMapper.toEntity(canonical))) }) return false
        }
        var reconciled = false
        val committed = commit(session) {
            val now = clock.instant()
            storage.reconcile(items, now)
            unresolvedFinalAnswer = null
            lastSuccess = now
            // Connectivity can change while the database transaction suspends. Its epoch is
            // independent of credential identity, and availability alone is never reconciliation.
            reconciled = connectionIsCurrent(session)
            if (reconciled) reconciledConnectionEpoch = session.connectionEpoch
            state.value = if (reconciled) SyncState.Current(now)
                else SyncState.Stale(now, ApiError.Transport("network_changed"))
        }
        return committed && reconciled
    }

    private suspend fun mutationFailed(session: Session, id: String, error: Exception, finalAnswer: Boolean = false) {
        handleFailure(session, error)
        if (error is ApiError.Transport || error is ApiError.AlreadyClosed || error is ApiError.Server) {
            // A write may have committed remotely. Never replay it; establish canonical state first.
            if (!isCurrent(session)) return
            try {
                if (finalAnswer) {
                    if (!commit(session) { unresolvedFinalAnswer = id }) return
                } else if (error is ApiError.AlreadyClosed) {
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
                val network = connectivity.value
                if (paired != null && (!network.available || state.value is SyncState.Current && reconciledConnectionEpoch != network.epoch)) {
                    state.value = SyncState.Stale(lastSuccess, ApiError.Transport("network_unavailable"))
                }
                if (paired == null || !network.available || (mutation && !state.value.mutationsEnabled)) null
                else apiProvider()?.let { Session(generation, it, network.epoch) }
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
        if (error !is CancellationException) commit(session) {
            recentErrors.addLast(errorCategory(error))
            if (recentErrors.size > 20) recentErrors.removeFirst()
        }
        if (error is ApiError.Unauthorized) {
            withContext(NonCancellable) {
                credentialChanges.withLock {
                    try {
                        clearSession(revoked = true, expected = session)
                    } catch (cleanupError: SessionCleanupException) {
                        errorEvents.trySend(cleanupError)
                    }
                }
            }
        } else {
            commit(session) { state.value = SyncState.Stale(lastSuccess, error) }
        }
        if (emit && isCurrent(session)) errorEvents.trySend(error)
    }

    private fun invalidateSessionLocked(revoked: Boolean) {
        generation++
        session.value = null
        paired = null
        lastSuccess = null
        reconciledConnectionEpoch = null
        recentErrors.clear()
        unresolvedFinalAnswer = null
        observedItems.clear()
        state.value = if (revoked) SyncState.Revoked else SyncState.Idle
    }

    // The caller holds credentialChanges, so teardown cannot erase a concurrent replacement.
    private suspend fun clearSession(revoked: Boolean, expected: Session? = null) {
        val failures = mutableListOf<Exception>()
        val invalidated = stateLock.withLock {
            if (expected != null && expected.generation != generation) false else {
                invalidateSessionLocked(revoked)
                pairing.tryEmit(null)
                try {
                    storage.clear()
                } catch (error: Exception) {
                    failures += error
                }
                true
            }
        }
        if (!invalidated) return
        attemptCredentialClear(failures)
        if (failures.isNotEmpty()) throw SessionCleanupException(failures)
    }

    private suspend fun attemptCredentialClear(failures: MutableList<Exception>) {
        try {
            credentials.clear()
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Exception) {
            failures += error
        }
    }

    private suspend fun isCurrent(session: Session): Boolean = stateLock.withLock { session.generation == generation }

    private fun connectionIsCurrent(session: Session): Boolean = connectivity.value.let { it.available && it.epoch == session.connectionEpoch }

    private suspend fun prepareSubmission(session: Session, id: String): Boolean = stateLock.withLock {
        val allowed = session.generation == generation && connectionIsCurrent(session) &&
            reconciledConnectionEpoch == session.connectionEpoch && state.value.mutationsEnabled
        // Cancellation after this point cannot prove the server did not receive the write.
        if (allowed) unresolvedFinalAnswer = id
        allowed
    }

    private suspend fun commit(session: Session, write: suspend () -> Unit): Boolean = stateLock.withLock {
        if (session.generation != generation) false else {
            write()
            true
        }
    }

    private data class Session(val generation: Long, val api: LamApi, val connectionEpoch: Long)
}

internal class SessionCleanupException(failures: List<Exception>) :
    Exception("Could not completely update local pairing data") {
    init { failures.forEach(::addSuppressed) }
}
