package dev.carraes.lam

import android.content.Context
import dev.carraes.lam.items.LamApi
import dev.carraes.lam.items.OkHttpLamApi
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import dev.carraes.lam.security.createKeystoreCredentialStore
import java.util.concurrent.atomic.AtomicReference
import okhttp3.HttpUrl.Companion.toHttpUrl

class AppContainer(
    val applicationContext: Context,
) {
    private val credential = AtomicReference<String?>(null)
    private val keystoreCredentialStore = createKeystoreCredentialStore(
        context = applicationContext,
        onCredentialChanged = credential::set,
    )

    val credentialStore: CredentialStore = keystoreCredentialStore

    internal fun authenticatedApi(server: PairedServer): LamApi = OkHttpLamApi(
        baseUrl = server.serverUrl.toHttpUrl(),
        credentialProvider = credential::get,
    )
}
