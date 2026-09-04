package dev.carraes.lam.security

import android.content.Context
import android.security.keystore.KeyInfo
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.test.core.app.ApplicationProvider
import java.io.File
import java.security.KeyStore
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
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
        val store = KeystoreCredentialStore(context)

        store.save(expected, CREDENTIAL)

        assertEquals(expected, store.observe().first())
        assertEquals(CREDENTIAL, store.credential())

        val recreated = KeystoreCredentialStore(context)
        assertEquals(expected, recreated.observe().first())
        assertEquals(CREDENTIAL, recreated.credential())

        recreated.clear()
        assertNull(recreated.observe().first())
        assertNull(recreated.credential())
        assertTrue(credentialsPreferences().all.isEmpty())
    }

    @Test
    fun normalizesAndPersistsServerAndDeviceMetadataWithoutPlaintextCredential() = runBlocking {
        val store = KeystoreCredentialStore(context)

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
        val store = KeystoreCredentialStore(context)
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
            KeystoreCredentialStore(context).save(pairedServer(), CREDENTIAL)
            credentialsPreferences().edit().putString(corruptedKey, "not-base64!").commit()

            val recreated = KeystoreCredentialStore(context)

            assertNull(recreated.observe().first())
            assertNull(recreated.credential())
            assertTrue(credentialsPreferences().all.isEmpty())
        }

        KeystoreCredentialStore(context).save(pairedServer(), CREDENTIAL)
        androidKeyStore().deleteEntry(defaultAlias())

        val recreatedAfterKeyLoss = KeystoreCredentialStore(context)

        assertNull(recreatedAfterKeyLoss.observe().first())
        assertNull(recreatedAfterKeyLoss.credential())
        assertTrue(credentialsPreferences().all.isEmpty())
    }

    @Test
    fun ciphertextIsBoundToTheNormalizedServerUrl() = runBlocking {
        KeystoreCredentialStore(context).save(pairedServer(), CREDENTIAL)
        credentialsPreferences().edit()
            .putString(KEY_SERVER_URL, "https://other.example/")
            .commit()

        val copiedToAnotherServer = KeystoreCredentialStore(context)

        assertNull(copiedToAnotherServer.observe().first())
        assertNull(copiedToAnotherServer.credential())
        assertTrue(credentialsPreferences().all.isEmpty())
    }

    @Test
    fun ciphertextCannotCrossDebugAndReleaseApplicationIds() = runBlocking {
        assertEquals("dev.carraes.lam.debug", context.packageName)
        KeystoreCredentialStore(context).save(pairedServer(), CREDENTIAL)

        val simulatedRelease = KeystoreCredentialStore(
            context = context,
            aadApplicationId = "dev.carraes.lam",
        )

        assertNull(simulatedRelease.observe().first())
        assertNull(simulatedRelease.credential())
        assertTrue(credentialsPreferences().all.isEmpty())
    }

    @Test
    fun ciphertextAndIvAreTheOnlyCredentialMaterialInPreferences() = runBlocking {
        KeystoreCredentialStore(context).save(pairedServer(), CREDENTIAL)
        val values = credentialsPreferences().all

        assertEquals(
            setOf(KEY_SERVER_URL, KEY_DEVICE_ID, KEY_DEVICE_NAME, KEY_IV, KEY_CIPHERTEXT),
            values.keys,
        )
        assertTrue(Base64.decode(values.getValue(KEY_IV) as String, Base64.NO_WRAP).isNotEmpty())
        assertTrue(Base64.decode(values.getValue(KEY_CIPHERTEXT) as String, Base64.NO_WRAP).isNotEmpty())
        assertFalse(values.values.any { value -> value.toString().contains(CREDENTIAL) })
    }

    private suspend fun eraseTestState() {
        KeystoreCredentialStore(context).clear()
        credentialsPreferences().edit().clear().commit()
        androidKeyStore().deleteEntry(defaultAlias())
    }

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
