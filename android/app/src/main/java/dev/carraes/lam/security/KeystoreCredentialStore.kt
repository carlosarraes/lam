package dev.carraes.lam.security

import android.annotation.SuppressLint
import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
import dev.carraes.lam.items.LamApi
import dev.carraes.lam.items.OkHttpLamApi
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.util.concurrent.atomic.AtomicReference
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl.Companion.toHttpUrl
import okhttp3.OkHttpClient

private data class PersistedCredential(
    val server: PairedServer,
    val credential: String,
)

private interface CredentialPersistence {
    fun restore(): PersistedCredential?

    fun save(server: PairedServer, credential: String)

    fun clear()

    fun recoverFromUnreadableRecord()
}

private class CredentialStoreRuntime(
    private val persistence: CredentialPersistence,
    private val ioDispatcher: CoroutineDispatcher,
    scope: CoroutineScope,
    private val onCredentialChanged: (PersistedCredential?) -> Unit,
) : CredentialStore {
    private val state = MutableSharedFlow<PairedServer?>(replay = 1)
    private val initialized = CompletableDeferred<Unit>()
    private val lock = Mutex()

    init {
        scope.launch(ioDispatcher) {
            try {
                lock.withLock {
                    val restored = try {
                        persistence.restore()
                    } catch (_: Exception) {
                        runCatching { persistence.recoverFromUnreadableRecord() }
                        null
                    }
                    onCredentialChanged(restored)
                    state.emit(restored?.server)
                }
            } finally {
                initialized.complete(Unit)
            }
        }
    }

    override fun observe(): Flow<PairedServer?> = state.asSharedFlow()

    override suspend fun save(server: PairedServer, credential: String) {
        require(credential.isNotBlank()) { "credential must not be blank" }
        withContext(ioDispatcher) {
            initialized.await()
            lock.withLock {
                persistence.save(server, credential)
                onCredentialChanged(PersistedCredential(server, credential))
                state.emit(server)
            }
        }
    }

    override suspend fun clear() {
        withContext(ioDispatcher) {
            initialized.await()
            lock.withLock {
                clearCredentialState(
                    deleteCredentialMaterial = persistence::clear,
                    publishUnpaired = {
                        onCredentialChanged(null)
                        state.emit(null)
                    },
                )
            }
        }
    }
}

internal interface CredentialComposition {
    val credentialStore: CredentialStore

    fun api(): LamApi?
    fun articleApi(): dev.carraes.lam.articles.ArticleApi?
}

internal fun createCredentialComposition(
    context: Context,
    aadApplicationId: String = context.packageName,
    ioDispatcher: CoroutineDispatcher = Dispatchers.IO,
    scope: CoroutineScope = CoroutineScope(SupervisorJob() + ioDispatcher),
): CredentialComposition = DefaultCredentialComposition(
    context,
    aadApplicationId,
    ioDispatcher,
    scope,
)

private class DefaultCredentialComposition(
    context: Context,
    aadApplicationId: String,
    ioDispatcher: CoroutineDispatcher,
    scope: CoroutineScope,
) : CredentialComposition {
    private val pairedCredential = AtomicReference<PersistedCredential?>(null)
    private val baseClient = OkHttpClient()

    override val credentialStore: CredentialStore = KeystoreCredentialStore(
        context = context,
        aadApplicationId = aadApplicationId,
        ioDispatcher = ioDispatcher,
        scope = scope,
        onCredentialChanged = pairedCredential::set,
    )

    override fun api(): LamApi? {
        val snapshot = pairedCredential.get() ?: return null
        return OkHttpLamApi(
            baseUrl = snapshot.server.serverUrl.toHttpUrl(),
            credentialProvider = {
                pairedCredential.get()?.takeIf { it === snapshot }?.credential
            },
            baseClient = baseClient,
        )
    }

    override fun articleApi(): dev.carraes.lam.articles.ArticleApi? {
        val snapshot = pairedCredential.get() ?: return null
        return dev.carraes.lam.articles.OkHttpArticleApi(snapshot.server.serverUrl.toHttpUrl(),
            credential = { pairedCredential.get()?.takeIf { it === snapshot }?.credential }, baseClient = baseClient)
    }
}

private class KeystoreCredentialStore(
    context: Context,
    aadApplicationId: String,
    ioDispatcher: CoroutineDispatcher,
    scope: CoroutineScope,
    onCredentialChanged: (PersistedCredential?) -> Unit,
) : CredentialStore {
    private val runtime = CredentialStoreRuntime(
        persistence = KeystoreCredentialPersistence(context, aadApplicationId),
        ioDispatcher = ioDispatcher,
        scope = scope,
        onCredentialChanged = onCredentialChanged,
    )

    override fun observe(): Flow<PairedServer?> = runtime.observe()

    override suspend fun save(server: PairedServer, credential: String) {
        runtime.save(server.copy(serverUrl = normalizeServerUrl(server.serverUrl)), credential)
    }

    override suspend fun clear() = runtime.clear()
}

private class KeystoreCredentialPersistence(
    context: Context,
    private val aadApplicationId: String,
) : CredentialPersistence {
    private val preferences: SharedPreferences =
        context.getSharedPreferences(PREFERENCES_NAME, Context.MODE_PRIVATE)
    private val keyAlias = "$KEY_ALIAS_PREFIX${context.packageName}"

    override fun restore(): PersistedCredential? {
        if (preferences.all.isEmpty()) return null
        val server = PairedServer(
            serverUrl = normalizeServerUrl(requiredPreference(KEY_SERVER_URL)),
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
        val credential = String(cipher.doFinal(ciphertext), StandardCharsets.UTF_8)
        return PersistedCredential(server, credential)
    }

    override fun save(server: PairedServer, credential: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply {
            init(Cipher.ENCRYPT_MODE, getOrCreateKey())
            updateAAD(associatedData(server.serverUrl))
        }
        val ciphertext = cipher.doFinal(credential.toByteArray(StandardCharsets.UTF_8))
        val persisted = preferences.edit()
            .putString(KEY_SERVER_URL, server.serverUrl)
            .putString(KEY_DEVICE_ID, server.deviceId)
            .putString(KEY_DEVICE_NAME, server.deviceName)
            .putString(KEY_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .putString(KEY_CIPHERTEXT, Base64.encodeToString(ciphertext, Base64.NO_WRAP))
            .commit()
        if (!persisted) error("could not persist encrypted credential")
    }

    // The KTX edit helper returns Unit, but clear must surface a failed synchronous commit.
    @SuppressLint("UseKtx")
    override fun clear() {
        var failed = !preferences.edit().clear().commit()
        try {
            androidKeyStore().deleteEntry(keyAlias)
        } catch (_: Exception) {
            failed = true
        }
        if (failed) error("could not delete credential material")
    }

    override fun recoverFromUnreadableRecord() {
        runCatching { preferences.edit(commit = true) { clear() } }
        runCatching { androidKeyStore().deleteEntry(keyAlias) }
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

    private fun androidKeyStore(): KeyStore = KeyStore.getInstance(ANDROID_KEY_STORE).apply { load(null) }

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
