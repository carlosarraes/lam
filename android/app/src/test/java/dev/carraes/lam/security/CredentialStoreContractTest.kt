package dev.carraes.lam.security

import dev.carraes.lam.AppContainer
import java.lang.reflect.Modifier
import java.util.concurrent.Executors
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CredentialStoreContractTest {
    @Test
    fun `public app surfaces expose no credential reader or provider`() {
        val forbiddenOutput = { method: java.lang.reflect.Method ->
            method.parameterCount == 0 &&
                (method.returnType == String::class.java ||
                    method.returnType.name.startsWith("kotlin.jvm.functions.Function"))
        }
        assertFalse(CredentialStore::class.java.methods.any(forbiddenOutput))
        assertFalse(AppContainer::class.java.declaredMethods.any(forbiddenOutput))
        assertFalse(
            AppContainer::class.java.methods.any {
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
        val implementation = Class.forName("dev.carraes.lam.security.KeystoreCredentialStore")
        assertFalse(Modifier.isPublic(implementation.modifiers))
    }

    @Test
    fun `observe emits nothing until restore completes on the injected dispatcher`() = runTest {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val expected = PairedServer("https://lam.example/", "device-1", "S24 Ultra")
        val persistence = FakeCredentialPersistence(PersistedCredential(expected, "device-credential"))
        var capturedCredential: String? = null
        val runtime = CredentialStoreRuntime(
            persistence = persistence,
            ioDispatcher = dispatcher,
            scope = this,
            onCredentialChanged = { capturedCredential = it },
        )
        val firstValue = async { runtime.observe().first() }

        assertFalse(firstValue.isCompleted)
        testScheduler.runCurrent()

        assertEquals(expected, firstValue.await())
        assertEquals("device-credential", capturedCredential)
    }

    @Test
    fun `save and clear run on the injected IO dispatcher and serialize state`() {
        val executor = Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "credential-store-io")
        }
        val dispatcher = executor.asCoroutineDispatcher()
        val scope = CoroutineScope(SupervisorJob() + dispatcher)
        try {
            val persistence = FakeCredentialPersistence()
            val captured = mutableListOf<String?>()
            val runtime = CredentialStoreRuntime(persistence, dispatcher, scope, captured::add)
            val server = PairedServer("https://lam.example/", "device-1", "S24 Ultra")

            runBlocking {
                runtime.save(server, "device-credential")
                assertEquals(server, runtime.observe().first())
                runtime.clear()
                assertNull(runtime.observe().first())
            }

            assertTrue(persistence.operationThreads.isNotEmpty())
            assertTrue(persistence.operationThreads.all { it.startsWith("credential-store-io") })
            assertEquals(listOf(null, "device-credential", null), captured)
        } finally {
            scope.cancel()
            dispatcher.close()
            executor.shutdownNow()
        }
    }

    @Test
    fun `explicit clear fails closed with a sanitized error when deletion fails`() = runTest {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val persistence = FakeCredentialPersistence().apply { clearFailure = true }
        val captured = mutableListOf<String?>()
        val runtime = CredentialStoreRuntime(persistence, dispatcher, this, captured::add)
        testScheduler.runCurrent()

        val result = async { runCatching { runtime.clear() } }
        testScheduler.runCurrent()
        val error = result.await().exceptionOrNull()

        assertTrue(error is CredentialStoreException)
        assertEquals("could not clear device credential", error?.message)
        val thrown = requireNotNull(error)
        val failureMessages = generateSequence<Throwable>(thrown) { current ->
            current.cause?.takeUnless { it === current }
        }.map { it.message }.toList()
        assertTrue(failureMessages.isNotEmpty())
        assertTrue(failureMessages.all { it == "could not clear device credential" })
        assertNull(runtime.observe().first())
        assertEquals(null, captured.last())
    }
}

private class FakeCredentialPersistence(
    var restored: PersistedCredential? = null,
) : CredentialPersistence {
    val operationThreads = mutableListOf<String>()
    var clearFailure = false

    override fun restore(): PersistedCredential? {
        operationThreads += Thread.currentThread().name
        return restored
    }

    override fun save(server: PairedServer, credential: String) {
        operationThreads += Thread.currentThread().name
        restored = PersistedCredential(server, credential)
    }

    override fun clear() {
        operationThreads += Thread.currentThread().name
        if (clearFailure) error("backend detail must not escape")
        restored = null
    }

    override fun recoverFromUnreadableRecord() {
        operationThreads += Thread.currentThread().name
        restored = null
    }
}
