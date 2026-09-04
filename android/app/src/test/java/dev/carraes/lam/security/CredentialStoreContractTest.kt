package dev.carraes.lam.security

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Test

class CredentialStoreContractTest {
    @Test
    fun `save publishes metadata without exposing the credential`() = runTest {
        val store = InMemoryCredentialStore()
        val server = PairedServer(
            serverUrl = "https://lam.example/",
            deviceId = "device-1",
            deviceName = "Carlos's S24 Ultra",
        )

        store.save(server, "device-credential")

        assertEquals(server, store.observe().first())
        assertFalse(store.observe().first().toString().contains("device-credential"))
    }

    @Test
    fun `clear publishes unpaired and removes the internal credential`() = runTest {
        val state = InMemoryCredentialState()
        val store = InMemoryCredentialStore(state)
        store.save(
            PairedServer("https://lam.example/", "device-1", "S24 Ultra"),
            "device-credential",
        )

        store.clear()

        assertNull(store.observe().first())
        assertNull(store.credential())
    }

    @Test
    fun `a recreated store reads metadata and credential from the same backing state`() = runTest {
        val state = InMemoryCredentialState()
        InMemoryCredentialStore(state).save(
            PairedServer("https://lam.example/", "device-1", "S24 Ultra"),
            "device-credential",
        )

        val recreated = InMemoryCredentialStore(state)

        assertEquals("device-1", recreated.observe().first()?.deviceId)
        assertEquals("device-credential", recreated.credential())
    }
}

private class InMemoryCredentialState(
    var pairedServer: PairedServer? = null,
    var credential: String? = null,
)

private class InMemoryCredentialStore(
    private val state: InMemoryCredentialState = InMemoryCredentialState(),
) : CredentialStore, CredentialReader {
    private val pairedServer = MutableStateFlow(state.pairedServer)

    override fun observe(): Flow<PairedServer?> = pairedServer

    override suspend fun save(server: PairedServer, credential: String) {
        state.pairedServer = server
        state.credential = credential
        pairedServer.value = server
    }

    override suspend fun clear() {
        state.pairedServer = null
        state.credential = null
        pairedServer.value = null
    }

    override fun credential(): String? = state.credential
}
