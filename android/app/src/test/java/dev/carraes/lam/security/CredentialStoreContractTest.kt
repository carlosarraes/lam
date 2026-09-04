package dev.carraes.lam.security

import dev.carraes.lam.AppContainer
import java.lang.reflect.Modifier
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
    fun `production module surfaces expose no credential reader provider or callback`() {
        val returnsCredential = { method: java.lang.reflect.Method ->
            method.parameterCount == 0 &&
                (method.returnType == String::class.java ||
                    method.returnType.name.startsWith("kotlin.jvm.functions.Function"))
        }
        assertFalse(CredentialStore::class.java.declaredMethods.any(returnsCredential))
        assertFalse(AppContainer::class.java.declaredMethods.any(returnsCredential))
        assertFalse(
            AppContainer::class.java.declaredMethods.any {
                it.name.contains("credentialProvider", ignoreCase = true) ||
                    it.name.contains("credentialReader", ignoreCase = true)
            },
        )
        assertFalse(
            PairedServer::class.java.declaredFields.any {
                it.name.contains("credential", ignoreCase = true) ||
                    it.name.contains("token", ignoreCase = true) ||
                    it.name.contains("secret", ignoreCase = true)
            },
        )

        val fileFacade = Class.forName("dev.carraes.lam.security.KeystoreCredentialStoreKt")
        assertFalse(
            fileFacade.declaredMethods.any { method ->
                !Modifier.isPrivate(method.modifiers) &&
                    method.parameterTypes.any {
                        it.name.startsWith("kotlin.jvm.functions.Function")
                    }
            },
        )
        listOf(
            "dev.carraes.lam.security.KeystoreCredentialStore",
            "dev.carraes.lam.security.CredentialStoreRuntime",
            "dev.carraes.lam.security.PersistedCredential",
        ).forEach { className ->
            assertFalse(Modifier.isPublic(Class.forName(className).modifiers))
        }
    }

    @Test
    fun `save publishes metadata without exposing the credential`() = runTest {
        val store = InMemoryCredentialStore()
        val server = PairedServer("https://lam.example/", "device-1", "S24 Ultra")

        store.save(server, "device-credential")

        assertEquals(server, store.observe().first())
        assertFalse(store.observe().first().toString().contains("device-credential"))
    }

    @Test
    fun `clear publishes unpaired`() = runTest {
        val store = InMemoryCredentialStore()
        store.save(
            PairedServer("https://lam.example/", "device-1", "S24 Ultra"),
            "device-credential",
        )

        store.clear()

        assertNull(store.observe().first())
    }
}

private class InMemoryCredentialStore : CredentialStore {
    private val state = MutableStateFlow<PairedServer?>(null)

    override fun observe(): Flow<PairedServer?> = state

    override suspend fun save(server: PairedServer, credential: String) {
        state.value = server
    }

    override suspend fun clear() {
        state.value = null
    }
}
