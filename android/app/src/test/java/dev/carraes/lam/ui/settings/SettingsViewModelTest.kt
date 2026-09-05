package dev.carraes.lam.ui.settings

import dev.carraes.lam.items.*
import dev.carraes.lam.diagnostics.*
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.*
import kotlinx.coroutines.test.*
import org.junit.After
import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class SettingsViewModelTest {
    @Before fun setup() { Dispatchers.setMain(StandardTestDispatcher()) }
    @After fun teardown() { Dispatchers.resetMain() }

    @Test fun `failed revoke requires second confirmation and never retries automatically`() = runTest {
        val credentials = FakeCredentials()
        val api = FakeApi().apply { revokeError = ApiError.Transport("credential body secret") }
        val repo = DefaultItemRepository(MemoryStorage(), { api }, credentials, backgroundScope)
        repo.refresh()
        val vm = SettingsViewModel(repo, Diagnostics("1.0", "16", "Samsung S24"))
        runCurrent()
        vm.requestUnpair()
        assertEquals(UnpairConfirmation.REMOTE, vm.state.value.confirmation)
        assertNotNull(credentials.state.value)
        vm.confirmUnpair(); vm.confirmUnpair(); runCurrent()
        assertEquals(UnpairConfirmation.LOCAL_ERASE, vm.state.value.confirmation)
        assertNotNull(credentials.state.value)
        assertEquals(1, api.revocations)
        vm.cancelUnpair(); vm.confirmUnpair(); runCurrent()
        assertNotNull(credentials.state.value)
        vm.requestUnpair(); vm.confirmUnpair(); runCurrent()
        vm.confirmUnpair(); runCurrent()
        assertNull(credentials.state.value)
        assertTrue(repo.openItems.first().isEmpty())
        assertEquals(2, api.revocations)
    }

    @Test fun `replacement session dismisses a stale local erase confirmation`() = runTest {
        val credentials = FakeCredentials()
        val api = FakeApi().apply { revokeError = ApiError.Transport("offline") }
        val repo = DefaultItemRepository(MemoryStorage(), { api }, credentials, backgroundScope)
        repo.refresh()
        val vm = SettingsViewModel(repo, Diagnostics("1.0", "16", "Samsung S24"))
        runCurrent()
        vm.requestUnpair(); vm.confirmUnpair(); runCurrent()
        val device = requireNotNull(credentials.state.value)
        repo.credentialStore.save(device, "replacement credential")
        runCurrent()
        assertNull(vm.state.value.confirmation)
        vm.confirmUnpair(); runCurrent()
        assertEquals(device, credentials.state.value)
        assertEquals(1, api.revocations)
    }

    @Test fun `diagnostics include versions origin counts last sync and bounded categories without raw errors`() = runTest {
        val storage = MemoryStorage()
        val api = FakeApi()
        val repo = DefaultItemRepository(storage, { api }, FakeCredentials(), backgroundScope)
        repo.refresh()
        storage.upsert(listOf(ItemMapper.toEntity(ItemRepositoryTest.item("closed", StatusDto.RESOLVED))))
        api.readError = ApiError.Server(500, "Bearer SECRET FCM request body")
        repeat(25) { repo.refresh() }
        val data = repo.diagnosticData()
        val text = Diagnostics("1.0", "16", "Samsung S24").render(data.copy(serverUrl = "https://user:SECRET@example.com:8443/path/FCM?token=SECRET#body"))
        assertTrue(text.contains("1.0"))
        assertTrue(text.contains("Android: 16"))
        assertTrue(text.contains("Samsung S24"))
        assertTrue(text.contains("https://example.com:8443"))
        assertTrue(text.contains("Open items: 1"))
        assertTrue(text.contains("Closed items: 1"))
        assertTrue(text.contains(requireNotNull(data.lastSuccess).toString()))
        assertEquals(20, data.recentErrors.size)
        assertTrue(text.contains("SERVER"))
        listOf("SECRET", "FCM", "request body", "Bearer", "/path", "user:").forEach { assertFalse(text.contains(it)) }
        repo.unpair()
        assertTrue(repo.diagnosticData().recentErrors.isEmpty())
    }
}
