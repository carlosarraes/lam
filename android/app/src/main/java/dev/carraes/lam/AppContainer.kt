package dev.carraes.lam

import android.content.Context
import dev.carraes.lam.items.LamApi
import dev.carraes.lam.security.CredentialComposition
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import dev.carraes.lam.security.createCredentialComposition

class AppContainer(
    val applicationContext: Context,
) {
    private val credentials: CredentialComposition = createCredentialComposition(applicationContext)

    val credentialStore: CredentialStore = credentials.credentialStore

    internal fun authenticatedApi(server: PairedServer): LamApi = credentials.apiFor(server)
}
