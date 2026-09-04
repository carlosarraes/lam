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
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

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
    fun saveObserveClearAndProcessRecreation() = runBlocking {
        val expected = pairedServer()
        var capturedCredential: String? = null
        val store = newStore { capturedCredential = it }

        store.save(expected, CREDENTIAL)

        assertEquals(expected, store.observe().first())
        assertEquals(CREDENTIAL, capturedCredential)

        var restoredCredential: String? = null
        val recreated = newStore { restoredCredential = it }
        assertEquals(expected, recreated.observe().first())
        assertEquals(CREDENTIAL, restoredCredential)

        recreated.clear()
        assertNull(recreated.observe().first())
        assertNull(restoredCredential)
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

            var capturedCredential: String? = "not-cleared"
            val recreated = newStore { capturedCredential = it }

            assertNull(recreated.observe().first())
            assertNull(capturedCredential)
            assertTrue(credentialsPreferences().all.isEmpty())
            assertFalse(androidKeyStore().containsAlias(defaultAlias()))
        }

        newStore().save(pairedServer(), CREDENTIAL)
        androidKeyStore().deleteEntry(defaultAlias())

        var restoredAfterKeyLoss: String? = "not-cleared"
        val recreatedAfterKeyLoss = newStore { restoredAfterKeyLoss = it }

        assertNull(recreatedAfterKeyLoss.observe().first())
        assertNull(restoredAfterKeyLoss)
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun ciphertextIsBoundToTheNormalizedServerUrl() = runBlocking {
        newStore().save(pairedServer(), CREDENTIAL)
        credentialsPreferences().edit()
            .putString(KEY_SERVER_URL, "https://other.example/")
            .commit()

        var copiedCredential: String? = "not-cleared"
        val copiedToAnotherServer = newStore { copiedCredential = it }

        assertNull(copiedToAnotherServer.observe().first())
        assertNull(copiedCredential)
        assertTrue(credentialsPreferences().all.isEmpty())
        assertFalse(androidKeyStore().containsAlias(defaultAlias()))
    }

    @Test
    fun ciphertextCannotCrossDebugAndReleaseApplicationIds() = runBlocking {
        assertEquals("dev.carraes.lam.debug", context.packageName)
        newStore().save(pairedServer(), CREDENTIAL)

        var simulatedReleaseCredential: String? = "not-cleared"
        val simulatedRelease = newStore("dev.carraes.lam") { simulatedReleaseCredential = it }

        assertNull(simulatedRelease.observe().first())
        assertNull(simulatedReleaseCredential)
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
    fun keystoreOperationsLeaveTheCallingMainThread() = runBlocking {
        val executor = Executors.newSingleThreadExecutor { runnable ->
            Thread(runnable, "credential-device-io")
        }
        val dispatcher = executor.asCoroutineDispatcher()
        val scope = CoroutineScope(SupervisorJob() + dispatcher)
        try {
            var credentialCallbackThread: Thread? = null
            val store = createKeystoreCredentialStore(
                context = context,
                ioDispatcher = dispatcher,
                scope = scope,
                onCredentialChanged = { credential ->
                    if (credential != null) credentialCallbackThread = Thread.currentThread()
                },
            )

            withContext(Dispatchers.Main) {
                store.save(pairedServer(), CREDENTIAL)
            }

            assertEquals("credential-device-io", credentialCallbackThread?.name)
            assertNotEquals(android.os.Looper.getMainLooper().thread, credentialCallbackThread)
        } finally {
            scope.cancel()
            dispatcher.close()
            executor.shutdownNow()
        }
    }

    private suspend fun eraseTestState() {
        newStore().clear()
        credentialsPreferences().edit().clear().commit()
        androidKeyStore().deleteEntry(defaultAlias())
    }

    private fun newStore(
        aadApplicationId: String = context.packageName,
        onCredentialChanged: (String?) -> Unit = {},
    ): CredentialStore = createKeystoreCredentialStore(
        context = context,
        aadApplicationId = aadApplicationId,
        onCredentialChanged = onCredentialChanged,
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
