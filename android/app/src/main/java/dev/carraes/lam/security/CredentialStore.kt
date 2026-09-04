package dev.carraes.lam.security

import kotlinx.coroutines.flow.Flow

interface CredentialStore {
    fun observe(): Flow<PairedServer?>

    suspend fun save(server: PairedServer, credential: String)

    suspend fun clear()
}

internal fun interface CredentialReader {
    fun credential(): String?
}
