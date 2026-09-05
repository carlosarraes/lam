package dev.carraes.lam.pairing

import dev.carraes.lam.items.*
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import java.io.IOException
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class PairingViewModelTest {
    @Before fun mainDispatcher() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun resetDispatcher() { Dispatchers.resetMain() }

    @Test fun validatesWorkerV1PayloadAndRedactsSecret() {
        val payload = PairingPayload.parse(qr(), false)
        assertEquals("https://lam.example", payload.serverUrl)
        assertFalse(payload.toString().contains(SECRET))
        payload.clearSecret()
        assertNull(payload.secret)
    }

    @Test fun rejectsMalformedUnsupportedAndUnsafePayloads() {
        val values = listOf("no json", qr().replace("\"v\":1", "\"v\":2"),
            qr("http://lam.example"), qr("https://user@lam.example"),
            qr("https://lam.example/?q=1"), qr("https://lam.example/#fragment"),
            qr("https://lam.example/path"), qr().replace(SECRET, "short"))
        values.forEach { value -> assertThrows(PairingPayloadException::class.java) { PairingPayload.parse(value, false) } }
        assertEquals(PairingProblem.UNSUPPORTED_VERSION, assertThrows(PairingPayloadException::class.java) {
            PairingPayload.parse(qr().replace("\"v\":1", "\"v\":2"), false)
        }.problem)
    }

    @Test fun debugHttpIsRestrictedToLocalAddresses() {
        listOf("127.0.0.1", "10.0.2.2", "192.168.1.4", "localhost", "[::1]").forEach {
            assertTrue(PairingPayload.parse(qr("http://$it:8787"), true).serverUrl.startsWith("http://"))
        }
        listOf("example.com", "8.8.8.8", "192.168.1.4.evil.example", "localhost.evil.example").forEach {
            assertThrows(PairingPayloadException::class.java) { PairingPayload.parse(qr("http://$it"), true) }
        }
    }

    @Test fun savesBeforeRefreshAndDuplicatesClaimOnlyOnce() = runTest {
        val store = Store()
        var calls = 0
        val gate = CompletableDeferred<Unit>()
        val repo = repository(store, refresh = { assertEquals("device-credential", store.credential); true }) { _, session, request ->
            calls++
            assertEquals(SESSION, session)
            assertEquals(SECRET, request.secret)
            assertNull(request.fcmToken)
            gate.await()
            response()
        }
        val vm = PairingViewModel(repo, false)
        runCurrent()
        vm.permissionResult(true)
        vm.onQr(qr()); vm.onQr(qr())
        runCurrent()
        assertEquals(1, calls)
        assertEquals(PairingState.Claiming, vm.state.value)
        gate.complete(Unit)
        advanceUntilIdle()
        assertEquals(PairedServer("https://lam.example", "device-1", "Pixel"), store.server.value)
        assertEquals(PairingState.Paired(false), vm.state.value)
    }

    @Test fun claimFailuresHaveDistinctSafeMessagesAndRequireHumanRetry() = runTest {
        val failures = listOf(
            ApiError.Server(409, null, ApiConflictCode.PAIRING_EXPIRED) to PairingProblem.EXPIRED,
            ApiError.Server(409, null, ApiConflictCode.PAIRING_CONSUMED) to PairingProblem.CONSUMED,
            ApiError.Server(409, null, ApiConflictCode.PAIRING_CANCELLED) to PairingProblem.CANCELLED,
            ApiError.Transport("SocketTimeoutException") to PairingProblem.OFFLINE,
            ApiError.Server(404, null) to PairingProblem.WRONG_SERVER,
            ApiError.Server(200, null) to PairingProblem.WRONG_SERVER,
            ApiError.Unauthorized(null) to PairingProblem.WRONG_SERVER,
            ApiError.Server(500, null) to PairingProblem.SERVER,
        )
        failures.forEach { (failure, expected) ->
            val store = Store(); var calls = 0
            val vm = PairingViewModel(repository(store) { _, _, _ -> calls++; throw failure }, false)
            runCurrent(); vm.permissionResult(true); vm.onQr(qr()); advanceUntilIdle()
            assertEquals(PairingState.Failed(expected), vm.state.value)
            vm.onQr(qr()); advanceUntilIdle()
            assertEquals(1, calls)
            assertNull(store.credential)
        }
    }

    @Test fun rejectsMismatchedOrIncompleteDeviceMetadataBeforeSaving() = runTest {
        val valid = response()
        listOf(valid.copy(credential = ""), valid.copy(device = valid.device.copy(id = "")),
            valid.copy(device = valid.device.copy(name = "Other")),
            valid.copy(device = valid.device.copy(appVersion = "wrong")),
            valid.copy(device = valid.device.copy(androidVersion = "wrong")),
            valid.copy(device = valid.device.copy(createdAt = "invalid")),
            valid.copy(device = valid.device.copy(pushRegistered = true))).forEach { result ->
            val store = Store()
            val payload = PairingPayload.parse(qr(), false)
            assertEquals(PairingProblem.WRONG_SERVER, repository(store) { _, _, _ -> result }.pair(payload).problem)
            assertNull(payload.secret)
            assertNull(store.credential)
        }
    }

    @Test fun saveFailureNeverRefreshesAndRequiresNewCode() = runTest {
        val store = Store(failSave = true)
        val result = repository(store, refresh = { error("must not refresh") }).pair(PairingPayload.parse(qr(), false))
        assertEquals(PairingProblem.STORAGE, result.problem)
        assertNull(store.credential)
    }

    @Test fun refreshFailureStaysPairedAndRetryDoesNotClaimAgain() = runTest {
        val store = Store(); var refreshes = 0; var claims = 0
        val vm = PairingViewModel(repository(store, refresh = { ++refreshes > 1 }) { _, _, _ -> claims++; response() }, false)
        runCurrent(); vm.permissionResult(true); vm.onQr(qr()); advanceUntilIdle()
        assertEquals(PairingState.Paired(true), vm.state.value)
        assertNotNull(store.credential)
        vm.retryRefresh(); advanceUntilIdle()
        assertEquals(PairingState.Paired(false), vm.state.value)
        assertEquals(1, claims)
    }

    @Test fun waitsForRestorationAndShowsExistingPairing() = runTest {
        val store = Store(emitInitial = false)
        val vm = PairingViewModel(repository(store), false)
        runCurrent(); assertEquals(PairingState.Loading, vm.state.value)
        store.events.emit(PairedServer("https://lam.example", "device-1", "Pixel"))
        runCurrent(); assertEquals(PairingState.Paired(true), vm.state.value)
    }

    @Test fun thrownRefreshFailureKeepsSavedCredential() = runTest {
        val store = Store()
        val result = repository(store, refresh = { throw IOException("offline") }).pair(PairingPayload.parse(qr(), false))
        assertTrue(result.stale)
        assertNull(result.problem)
        assertNotNull(store.credential)
    }

    @Test fun revokedDuringFirstRefreshReturnsToPairing() = runTest {
        val store = FakeCredentials().apply { state.value = null; publishCurrent() }
        val api = FakeApi().apply { readError = ApiError.Unauthorized(null) }
        val items = DefaultItemRepository(MemoryStorage(), { api }, store, backgroundScope)
        val pairing = PairingRepository(items.credentialStore, items::refresh, DeviceIdentity("Pixel", "0.1.0", "16"),
            { _, _, _ -> response() })
        val vm = PairingViewModel(pairing, false, items.syncState)
        runCurrent(); vm.permissionResult(true); vm.onQr(qr()); advanceUntilIdle()
        assertEquals(PairingState.Revoked, vm.state.value)
        assertNull(store.state.value)
    }

    @Test fun existingSessionUnauthorizedExplainsRevocationButVoluntaryUnpairDoesNot() = runTest {
        val store = FakeCredentials()
        val api = FakeApi()
        val items = DefaultItemRepository(MemoryStorage(), { api }, store, backgroundScope)
        val pairing = PairingRepository(items.credentialStore, items::refresh, DeviceIdentity("Pixel", "0.1.0", "16"))
        val vm = PairingViewModel(pairing, false, items.syncState)
        runCurrent(); assertTrue(items.refresh())
        api.readError = ApiError.Unauthorized(null)
        assertFalse(items.refresh()); runCurrent()
        assertEquals(PairingState.Revoked, vm.state.value)
        items.credentialStore.save(PairedServer("https://lam.example", "replacement", "Pixel"), "synthetic")
        runCurrent(); items.unpair(); runCurrent()
        assertEquals(PairingState.Ready, vm.state.value)
    }

    @Test fun permissionDenialCanRetryAndCancelIgnoresLateFrames() = runTest {
        val vm = PairingViewModel(repository(Store()) { _, _, _ -> error("must not claim") }, false)
        runCurrent(); vm.permissionResult(false)
        assertEquals(PairingState.PermissionDenied, vm.state.value)
        vm.permissionResult(true); assertEquals(PairingState.Scanning, vm.state.value)
        vm.cancelScan(); vm.onQr(qr()); runCurrent()
        assertEquals(PairingState.Ready, vm.state.value)
    }

    @Test fun cancelledClaimDropsSecretWithoutSaving() = runTest {
        val payload = PairingPayload.parse(qr(), false)
        val store = Store()
        val repo = repository(store) { _, _, _ -> awaitCancellation() }
        val job = launch { repo.pair(payload) }
        runCurrent(); job.cancelAndJoin()
        assertNull(payload.secret)
        assertNull(store.credential)
    }

    private fun repository(store: Store, refresh: suspend () -> Boolean = { true },
        claim: suspend (String, String, PairingClaimRequestDto) -> PairingClaimResponseDto = { _, _, _ -> response() },
    ) = PairingRepository(store, refresh, DeviceIdentity("Pixel", "0.1.0", "16"), claim)

    private class Store(val failSave: Boolean = false, emitInitial: Boolean = true) : CredentialStore {
        val server = MutableStateFlow<PairedServer?>(null)
        val events = MutableSharedFlow<PairedServer?>(replay = 1).apply { if (emitInitial) tryEmit(null) }
        var credential: String? = null
        override fun observe(): Flow<PairedServer?> = events
        override suspend fun save(server: PairedServer, credential: String) {
            if (failSave) throw IOException("disk")
            this.credential = credential; this.server.value = server; events.emit(server)
        }
        override suspend fun clear() { credential = null; server.value = null; events.emit(null) }
    }

    companion object {
        const val SECRET = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        const val SESSION = "550e8400-e29b-41d4-a716-446655440000"
        fun qr(server: String = "https://lam.example/") = """{"v":1,"server":"$server","session":"$SESSION","secret":"$SECRET"}"""
        fun response() = PairingClaimResponseDto("device-credential", DeviceRegistrationDto(
            "device-1", "Pixel", "0.1.0", "16", "2026-09-04T00:00:00Z", null, false,
        ))
    }
}
