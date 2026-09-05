package dev.carraes.lam

import android.content.Context
import androidx.room.Room
import dev.carraes.lam.items.DefaultItemRepository
import dev.carraes.lam.items.ItemRepository
import dev.carraes.lam.items.LamApi
import dev.carraes.lam.items.LamDatabase
import dev.carraes.lam.items.RoomItemStorage
import dev.carraes.lam.security.CredentialComposition
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.createCredentialComposition
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob

class AppContainer(
    val applicationContext: Context,
) {
    private val credentials: CredentialComposition = createCredentialComposition(applicationContext)
    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val database = Room.databaseBuilder(applicationContext, LamDatabase::class.java, "lam-items.db").build()

    val itemRepository: ItemRepository = DefaultItemRepository(
        RoomItemStorage(database), ::authenticatedApi, credentials.credentialStore, applicationScope,
    )

    val credentialStore: CredentialStore = object : CredentialStore by credentials.credentialStore {
        override suspend fun clear() = itemRepository.unpair()
    }

    internal fun authenticatedApi(): LamApi? = credentials.api()
}
