package dev.carraes.lam.security

import android.content.Context
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.test.core.app.ApplicationProvider
import java.io.File
import java.security.KeyStore
import java.util.concurrent.Executors
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.asCoroutineDispatcher
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.test.StandardTestDispatcher
import kotlinx.coroutines.test.runTest
import kotlinx.coroutines.withContext
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import androidx.room.Room
import dev.carraes.lam.articles.*
import dev.carraes.lam.items.DefaultItemRepository
import dev.carraes.lam.items.LamDatabase
import dev.carraes.lam.items.RoomItemStorage
import dev.carraes.lam.items.SyncState
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.JsonPrimitive

class KeystoreCredentialStoreTest {
    private lateinit var context: Context

    @Before
    fun setUp() = runBlocking {
        context = ApplicationProvider.getApplicationContext()
        eraseTestState()
    }

    @After
    fun tearDown() = runBlocking {
        eraseTestState()
    }

    @Test
    fun article401ClearsRoomAndKeystoreButOldSnapshotCannotEraseReplacement() = runBlocking {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
        val database = Room.inMemoryDatabaseBuilder(context, LamDatabase::class.java).build()
        val composition = createCredentialComposition(context, scope = scope)
        try {
            MockWebServer().use { server ->
                val items = DefaultItemRepository(RoomItemStorage(database), composition::api, composition.credentialStore, scope)
                val paired = pairedServer().copy(serverUrl = server.url("/").toString())
                items.credentialStore.save(paired, CREDENTIAL)
                val binding = ArticleSessionBinding(items.pairedSession, composition::articleApi)
                val cache = RoomArticleStorage(database)
                val repository = ArticleRepository(cache, binding::api, binding::current) { items.rejectCredential(it.generation) }
                val article = Article("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Private", "Summary", "Agent", "host", "lam",
                    "2026-09-08T00:00:00Z", null, 0, listOf(ArticleAsset("index.html", "text/html", 1, "0".repeat(64), "inline")))
                val body = articleJson.encodeToString(ArticleContent(article, listOf(JsonPrimitive("<p>Private content</p>"))))
                server.enqueue(MockResponse().setBody(body))
                val loaded = requireNotNull(repository.load(article.id))
                val oldApi = requireNotNull(binding.api(loaded.session))
                assertEquals("Bearer $CREDENTIAL", server.takeRequest().getHeader("Authorization"))
                server.enqueue(MockResponse().setResponseCode(401))
                assertNull(repository.load(article.id))
                assertEquals(2, server.requestCount)
                assertTrue(cache.list(loaded.session.account).isEmpty())
                assertNull(binding.current())
                assertNull(composition.credentialStore.observe().first())
                assertEquals(SyncState.Revoked, items.syncState.value)

                items.credentialStore.save(paired, "synthetic-article-replacement")
                server.enqueue(MockResponse().setBody(body))
                val replacement = requireNotNull(repository.load(article.id))
                items.rejectCredential(loaded.session.generation)
                assertTrue(repository.isCurrent(replacement.session))
                assertFalse(cache.list(replacement.session.account).isEmpty())
                assertEquals(paired, composition.credentialStore.observe().first())
                val failure = runCatching { oldApi.content(article.id) }.exceptionOrNull()
                assertTrue(failure is ArticleHttpException && failure.status == 401)
                assertEquals("Old client cannot send the replacement credential", 3, server.requestCount)
            }
        } finally {
            composition.credentialStore.clear()
            scope.cancel()
            database.close()
        }
    }

    @Test
    fun saveObserveClearAndProcessRecreation() = runBlocking {
        val expected = pairedServer()
        val store = newStore()

        store.save(expected, CREDENTIAL)

        assertEquals(expected, store.observe().first())

        val recreated = newStore()
        assertEquals(expected, recreated.observe().first())

        recreated.clear()
        assertNull(recreated.observe().first())
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun normalizesAndPersistsServerAndDeviceMetadataWithoutPlaintextCredential() = runBlocking {
        val store = newStore()

        store.save(
            PairedServer("HTTPS://Lam.Example:443/worker", "device-42", "Carlos's S24 Ultra"),
            CREDENTIAL,
        )

        val paired = store.observe().first()
        assertEquals("https://lam.example/worker/", paired?.serverUrl)
        assertEquals("device-42", paired?.deviceId)
        assertEquals("Carlos's S24 Ultra", paired?.deviceName)
        assertFalse(paired.toString().contains(CREDENTIAL))

        val preferencesText = preferencesFile().readText()
        assertFalse(preferencesText.contains(CREDENTIAL))
        assertFalse(credentialsPreferences().all.values.any { it == CREDENTIAL })
        assertTrue(credentialsPreferences().contains(KEY_IV))
        assertTrue(credentialsPreferences().contains(KEY_CIPHERTEXT))
    }

    @Test
    fun usesRandomizedAes256GcmWithoutUserAuthentication() = runBlocking {
        val store = newStore()
        store.save(pairedServer(), CREDENTIAL)
        val firstIv = credentialsPreferences().getString(KEY_IV, null)
        val firstCiphertext = credentialsPreferences().getString(KEY_CIPHERTEXT, null)

        store.save(pairedServer(), CREDENTIAL)

        assertNotEquals(firstIv, credentialsPreferences().getString(KEY_IV, null))
        assertNotEquals(firstCiphertext, credentialsPreferences().getString(KEY_CIPHERTEXT, null))
        val key = androidKeyStore().getKey(defaultAlias(), null) as SecretKey
        val keyInfo = SecretKeyFactory.getInstance(key.algorithm, ANDROID_KEY_STORE)
            .getKeySpec(key, KeyInfo::class.java)
            as KeyInfo
        assertEquals(256, keyInfo.keySize)
        assertEquals(KeyProperties.KEY_ALGORITHM_AES, key.algorithm)
        assertTrue(keyInfo.blockModes.contains(KeyProperties.BLOCK_MODE_GCM))
        assertTrue(keyInfo.encryptionPaddings.contains(KeyProperties.ENCRYPTION_PADDING_NONE))
        assertFalse(keyInfo.isUserAuthenticationRequired)
    }

    @Test
    fun corruptedIvCiphertextOrMissingKeyErasesTheUnreadableRecord() = runBlocking {
        listOf(KEY_IV, KEY_CIPHERTEXT).forEach { corruptedKey ->
            eraseTestState()
            newStore().save(pairedServer(), CREDENTIAL)
            credentialsPreferences().edit().putString(corruptedKey, "not-base64!").commit()

            val recoveredComposition = newComposition()
            val recreated = recoveredComposition.credentialStore

            assertNull(recreated.observe().first())
            assertNull(recoveredComposition.api())
            assertTrue(credentialsPreferences().all.isEmpty())
            assertFalse(androidKeyStore().containsAlias(defaultAlias()))
        }

        newStore().save(pairedServer(), CREDENTIAL)
        androidKeyStore().deleteEntry(defaultAlias())

        val recoveredAfterKeyLoss = newComposition()
        val recreatedAfterKeyLoss = recoveredAfterKeyLoss.credentialStore

        assertNull(recreatedAfterKeyLoss.observe().first())
        assertNull(recoveredAfterKeyLoss.api())
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun ciphertextIsBoundToTheNormalizedServerUrl() = runBlocking {
        newStore().save(pairedServer(), CREDENTIAL)
        credentialsPreferences().edit()
            .putString(KEY_SERVER_URL, "https://other.example/")
            .commit()

        val copiedToAnotherServer = newStore()

        assertNull(copiedToAnotherServer.observe().first())
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun ciphertextCannotCrossDebugAndReleaseApplicationIds() = runBlocking {
        assertEquals("dev.carraes.lam.debug", context.packageName)
        newStore().save(pairedServer(), CREDENTIAL)

        val simulatedRelease = newStore("dev.carraes.lam")

        assertNull(simulatedRelease.observe().first())
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun ciphertextAndIvAreTheOnlyCredentialMaterialInPreferences() = runBlocking {
        newStore().save(pairedServer(), CREDENTIAL)
        val values = credentialsPreferences().all

        assertEquals(
            setOf(KEY_SERVER_URL, KEY_DEVICE_ID, KEY_DEVICE_NAME, KEY_IV, KEY_CIPHERTEXT),
            values.keys,
        )
        assertTrue(Base64.decode(values.getValue(KEY_IV) as String, Base64.NO_WRAP).isNotEmpty())
        assertTrue(Base64.decode(values.getValue(KEY_CIPHERTEXT) as String, Base64.NO_WRAP).isNotEmpty())
        assertFalse(values.values.any { value -> value.toString().contains(CREDENTIAL) })
    }

    @Test
    fun safeCompositionAuthenticatesWithoutExposingTheCredential() = runBlocking {
        MockWebServer().use { server ->
            repeat(2) {
                server.enqueue(MockResponse().setHeader("Content-Type", "application/json").setBody("[]"))
            }
            val composition = createCredentialComposition(context)
            val paired = pairedServer().copy(serverUrl = server.url("/").toString())

            assertNull(composition.api())
            composition.credentialStore.save(paired, CREDENTIAL)
            val authenticatedApi = requireNotNull(composition.api())
            authenticatedApi.listOpenItems()

            assertEquals("Bearer $CREDENTIAL", server.takeRequest().getHeader("Authorization"))
            assertFalse(
                composition.javaClass.declaredMethods.any { method ->
                    method.parameterCount == 0 &&
                        (method.returnType == String::class.java ||
                            method.returnType.name.startsWith("kotlin.jvm.functions.Function"))
                },
            )
            assertFalse(composition.credentialStore.observe().first().toString().contains(CREDENTIAL))

            val recreated = createCredentialComposition(context)
            assertEquals(paired, recreated.credentialStore.observe().first())
            requireNotNull(recreated.api()).listOpenItems()
            assertEquals("Bearer $CREDENTIAL", server.takeRequest().getHeader("Authorization"))

            recreated.credentialStore.clear()
            assertNull(recreated.api())
        }
    }

    @Test
    fun issuedClientHasNoAuthorizationAfterClearOrReplacement() = runBlocking {
        MockWebServer().use { server ->
            repeat(4) { server.enqueue(MockResponse().setHeader("Content-Type", "application/json").setBody("[]")) }
            val composition = createCredentialComposition(context)
            val paired = pairedServer().copy(serverUrl = server.url("/").toString())
            composition.credentialStore.save(paired, CREDENTIAL)
            val oldApi = requireNotNull(composition.api())
            oldApi.listOpenItems()
            assertEquals("Bearer $CREDENTIAL", server.takeRequest().getHeader("Authorization"))
            composition.credentialStore.clear()
            oldApi.listOpenItems()
            assertNull(server.takeRequest().getHeader("Authorization"))
            composition.credentialStore.save(paired, "synthetic-replacement-credential")
            oldApi.listOpenItems()
            assertNull(server.takeRequest().getHeader("Authorization"))
            requireNotNull(composition.api()).listOpenItems()
            assertEquals("Bearer synthetic-replacement-credential", server.takeRequest().getHeader("Authorization"))
        }
    }

    @Test
    fun keystoreOperationsLeaveTheCallingMainThread() = runBlocking {
        val executor = Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "credential-device-io")
        }
        val dispatcher = executor.asCoroutineDispatcher()
        val scope = CoroutineScope(SupervisorJob() + dispatcher)
        try {
            val recordingDispatcher = RecordingDispatcher(dispatcher)
            val store = createCredentialComposition(
                context = context,
                ioDispatcher = recordingDispatcher,
                scope = scope,
            ).credentialStore

            withContext(Dispatchers.Main) {
                store.save(pairedServer(), CREDENTIAL)
            }

            assertTrue(recordingDispatcher.threads.isNotEmpty())
            assertTrue(recordingDispatcher.threads.all { it.name == "credential-device-io" })
            assertFalse(recordingDispatcher.threads.contains(android.os.Looper.getMainLooper().thread))
        } finally {
            scope.cancel()
            dispatcher.close()
            executor.shutdownNow()
        }
    }

    @Test
    fun observeWaitsForDeterministicRestoreBeforeEmittingUnpaired() = runTest {
        val dispatcher = StandardTestDispatcher(testScheduler)
        val composition = createCredentialComposition(
            context = context,
            ioDispatcher = dispatcher,
            scope = this,
        )
        val first = async { composition.credentialStore.observe().first() }

        assertFalse(first.isCompleted)
        assertNull(composition.api())
        testScheduler.runCurrent()

        assertNull(first.await())
        assertNull(composition.api())
    }

    private suspend fun eraseTestState() {
        newStore().clear()
        credentialsPreferences().edit().clear().commit()
        androidKeyStore().deleteEntry(defaultAlias())
    }

    private fun newStore(
        aadApplicationId: String = context.packageName,
    ): CredentialStore = newComposition(aadApplicationId).credentialStore

    private fun newComposition(
        aadApplicationId: String = context.packageName,
    ): CredentialComposition = createCredentialComposition(
        context = context,
        aadApplicationId = aadApplicationId,
    )

    private fun pairedServer() = PairedServer(
        serverUrl = "https://lam.example/",
        deviceId = "device-42",
        deviceName = "Carlos's S24 Ultra",
    )

    private fun credentialsPreferences() = context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)

    private fun preferencesFile() = File(context.applicationInfo.dataDir, "shared_prefs/$PREFERENCES_NAME.xml")

    private fun androidKeyStore(): KeyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }

    private fun defaultAlias() = "lam.device-credential.${context.packageName}"

    private companion object {
        const val ANDROID_KEY_STORE = "AndroidKeyStore"
        const val PREFERENCES_NAME = "credentials"
        const val KEY_SERVER_URL = "server_url"
        const val KEY_DEVICE_ID = "device_id"
        const val KEY_DEVICE_NAME = "device_name"
        const val KEY_IV = "credential_iv"
        const val KEY_CIPHERTEXT = "credential_ciphertext"
        const val CREDENTIAL = "test-device-credential-never-store-plaintext"
    }
}

private class RecordingDispatcher(
    private val delegate: kotlinx.coroutines.CoroutineDispatcher,
) : kotlinx.coroutines.CoroutineDispatcher() {
    val threads = java.util.concurrent.CopyOnWriteArrayList<Thread>()

    override fun dispatch(context: kotlin.coroutines.CoroutineContext, block: Runnable) {
        delegate.dispatch(context) {
            threads += Thread.currentThread()
            block.run()
        }
    }
}
