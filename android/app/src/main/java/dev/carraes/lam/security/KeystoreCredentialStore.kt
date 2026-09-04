package dev.carraes.lam.security

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
import java.io.IOException
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import okhttp3.HttpUrl.Companion.toHttpUrl

class KeystoreCredentialStore internal constructor(
    context: Context,
    private val aadApplicationId: String = context.packageName,
) : CredentialStore, CredentialReader {
    private val preferences: SharedPreferences =
        context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
    private val keyAlias = "$KEY_ALIAS_PREFIX${context.packageName}"
    private val state = MutableStateFlow<PairedServer?>(null)
    private val lock = Mutex()

    @Volatile
    private var currentCredential: String? = null

    init {
        restore()
    }

    override fun observe(): Flow<PairedServer?> = state

    override suspend fun save(server: PairedServer, credential: String) {
        require(credential.isNotBlank()) { "credential must not be blank" }
        lock.withLock {
            val normalizedServer = server.copy(serverUrl = normalizeServerUrl(server.serverUrl))
            val cipher = Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.ENCRYPT_MODE, getOrCreateKey())
                updateAAD(associatedData(normalizedServer.serverUrl))
            }
            val ciphertext = cipher.doFinal(credential.toByteArray(StandardCharsets.UTF_8))
            val persisted = preferences.edit()
                .putString(KEY_SERVER_URL, normalizedServer.serverUrl)
                .putString(KEY_DEVICE_ID, normalizedServer.deviceId)
                .putString(KEY_DEVICE_NAME, normalizedServer.deviceName)
                .putString(KEY_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
                .putString(KEY_CIPHERTEXT, Base64.encodeToString(ciphertext, Base64.NO_WRAP))
                .commit()
            if (!persisted) throw IOException("could not persist device credential")
            currentCredential = credential
            state.value = normalizedServer
        }
    }

    override suspend fun clear() {
        lock.withLock {
            eraseRecordAndKey()
        }
    }

    override fun credential(): String? = currentCredential

    private fun restore() {
        if (preferences.all.isEmpty()) return
        try {
            val serverUrl = requiredPreference(KEY_SERVER_URL)
            val server = PairedServer(
                serverUrl = normalizeServerUrl(serverUrl),
                deviceId = requiredPreference(KEY_DEVICE_ID),
                deviceName = requiredPreference(KEY_DEVICE_NAME),
            )
            val iv = Base64.decode(requiredPreference(KEY_IV), Base64.NO_WRAP)
            val ciphertext = Base64.decode(requiredPreference(KEY_CIPHERTEXT), Base64.NO_WRAP)
            val key = loadKey() ?: error("device credential key is missing")
            val cipher = Cipher.getInstance(TRANSFORMATION).apply {
                init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(GCM_TAG_BITS, iv))
                updateAAD(associatedData(server.serverUrl))
            }
            currentCredential = String(cipher.doFinal(ciphertext), StandardCharsets.UTF_8)
            state.value = server
        } catch (_: Exception) {
            eraseRecordAndKey()
        }
    }

    private fun requiredPreference(name: String): String =
        preferences.getString(name, null)?.takeIf(String::isNotBlank)
            ?: error("credential record is incomplete")

    private fun associatedData(serverUrl: String): ByteArray =
        "$aadApplicationId\n$serverUrl".toByteArray(StandardCharsets.UTF_8)

    private fun getOrCreateKey(): SecretKey = loadKey() ?: KeyGenerator
        .getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEY_STORE)
        .apply {
            init(
                KeyGenParameterSpec.Builder(
                    keyAlias,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setKeySize(KEY_SIZE_BITS)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setRandomizedEncryptionRequired(true)
                    .setUserAuthenticationRequired(false)
                    .setUnlockedDeviceRequired(false)
                    .build(),
            )
        }
        .generateKey()

    private fun loadKey(): SecretKey? = androidKeyStore().getKey(keyAlias, null) as? SecretKey

    private fun eraseRecordAndKey() {
        currentCredential = null
        state.value = null
        preferences.edit(commit = true) { clear() }
        runCatching { androidKeyStore().deleteEntry(keyAlias) }
    }

    private fun androidKeyStore(): KeyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }

    private fun normalizeServerUrl(value: String): String {
        val parsed = value.trim().toHttpUrl()
        require(parsed.username.isEmpty() && parsed.password.isEmpty()) {
            "server URL must not contain user information"
        }
        require(parsed.query == null && parsed.fragment == null) {
            "server URL must not contain a query or fragment"
        }
        return if (parsed.encodedPath.endsWith('/')) {
            parsed.toString()
        } else {
            parsed.newBuilder().addPathSegment("").build().toString()
        }
    }

    private companion object {
        const val ANDROID_KEY_STORE = "AndroidKeyStore"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val KEY_ALIAS_PREFIX = "lam.device-credential."
        const val KEY_SIZE_BITS = 256
        const val GCM_TAG_BITS = 128
        const val PREFERENCES_NAME = "credentials"
        const val KEY_SERVER_URL = "server_url"
        const val KEY_DEVICE_ID = "device_id"
        const val KEY_DEVICE_NAME = "device_name"
        const val KEY_IV = "credential_iv"
        const val KEY_CIPHERTEXT = "credential_ciphertext"
    }
}
