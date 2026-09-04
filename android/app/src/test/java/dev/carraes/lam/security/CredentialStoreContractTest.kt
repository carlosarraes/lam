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
        val acceptsDestinationOrTransport = { method: java.lang.reflect.Method ->
            method.parameterTypes.any {
                it.name == PairedServer::class.java.name ||
                    it.name.startsWith("okhttp3.")
            }
        }
        assertFalse(CredentialStore::class.java.declaredMethods.any(returnsCredential))
        assertFalse(AppContainer::class.java.declaredMethods.any(returnsCredential))
        assertFalse(AppContainer::class.java.declaredMethods.any(acceptsDestinationOrTransport))
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
                    method.parameterTypes.zip(method.genericParameterTypes).any { (raw, generic) ->
                        raw.name.startsWith("kotlin.jvm.functions.Function") &&
                            generic.typeName.contains("java.lang.String")
                    }
            },
        )
        assertFalse(
            fileFacade.declaredMethods.any { method ->
                !Modifier.isPrivate(method.modifiers) &&
                    acceptsDestinationOrTransport(method)
            },
        )
        val factory = fileFacade.declaredMethods.single {
            it.name == "createCredentialComposition"
        }
        assertEquals(
            listOf(
                "android.content.Context",
                "java.lang.String",
                "kotlinx.coroutines.CoroutineDispatcher",
                "kotlinx.coroutines.CoroutineScope",
            ),
            factory.parameterTypes.map { it.name },
        )
        val composition = Class.forName("dev.carraes.lam.security.CredentialComposition")
        assertFalse(composition.declaredMethods.any(acceptsDestinationOrTransport))
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

    @Test
    fun `explicit deletion failure is sanitized after state becomes unpaired`() = runTest {
        var unpaired = false

        val error = runCatching {
            clearCredentialState(
                deleteCredentialMaterial = { error("backend detail must not escape") },
                publishUnpaired = { unpaired = true },
            )
        }.exceptionOrNull()

        assertEquals("could not clear device credential", error?.message)
        assertFalse(error?.message.orEmpty().contains("backend detail"))
        assertEquals(true, unpaired)
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
