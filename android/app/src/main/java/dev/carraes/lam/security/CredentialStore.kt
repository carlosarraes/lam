package dev.carraes.lam.security

import kotlinx.coroutines.flow.Flow
import java.io.IOException

interface CredentialStore {
    fun observe(): Flow<PairedServer?>

    suspend fun save(server: PairedServer, credential: String)

    suspend fun clear()
}

internal class CredentialStoreException : IOException("could not clear device credential")
