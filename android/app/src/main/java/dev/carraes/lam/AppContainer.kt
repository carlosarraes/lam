package dev.carraes.lam

import android.content.Context
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.KeystoreCredentialStore

class AppContainer(
    val applicationContext: Context,
) {
    private val keystoreCredentialStore = KeystoreCredentialStore(applicationContext)

    val credentialStore: CredentialStore = keystoreCredentialStore

    internal val credentialProvider: () -> String? = keystoreCredentialStore::credential
}
