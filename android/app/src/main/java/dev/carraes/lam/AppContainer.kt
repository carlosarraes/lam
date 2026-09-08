package dev.carraes.lam

import android.content.Context
import androidx.room.Room
import androidx.lifecycle.ProcessLifecycleOwner
import dev.carraes.lam.items.DefaultItemRepository
import dev.carraes.lam.items.ItemRepository
import dev.carraes.lam.items.LamApi
import dev.carraes.lam.items.LamDatabase
import dev.carraes.lam.items.RoomItemStorage
import dev.carraes.lam.security.CredentialComposition
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.createCredentialComposition
import dev.carraes.lam.pairing.DeviceIdentity
import dev.carraes.lam.pairing.PairingRepository
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import dev.carraes.lam.sync.LifecycleReconciler
import dev.carraes.lam.sync.AndroidConnectivityMonitor
import dev.carraes.lam.items.DeviceSettings
import dev.carraes.lam.diagnostics.Diagnostics
import dev.carraes.lam.articles.*
import kotlinx.coroutines.launch

class AppContainer(
    val applicationContext: Context,
) {
    private val credentials: CredentialComposition = createCredentialComposition(applicationContext)
    private val applicationScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val database = Room.databaseBuilder(applicationContext, LamDatabase::class.java, "lam-items.db")
        .addMigrations(LamDatabase.MIGRATION_1_2, LamDatabase.MIGRATION_2_3).build()
    private val connectivity = AndroidConnectivityMonitor(applicationContext)

    private val items = DefaultItemRepository(
        RoomItemStorage(database), ::authenticatedApi, credentials.credentialStore, applicationScope,
        connectivity = connectivity.state,
    )

    val itemRepository: ItemRepository = items
    val deviceSettings: DeviceSettings = items
    val diagnostics = Diagnostics(BuildConfig.VERSION_NAME, android.os.Build.VERSION.RELEASE,
        "${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL} ${android.os.Build.ID}")

    val credentialStore: CredentialStore = items.credentialStore

    private val articleSessions = ArticleSessionBinding(items.pairedSession, credentials::articleApi)
    val articleRepository = ArticleRepository(RoomArticleStorage(database), articleSessions::api, articleSessions::current,
        onUnauthorized = { items.rejectCredential(it.generation) })

    init {
        applicationScope.launch {
            items.pairedSession.collect { articleRepository.reset() }
        }
    }

    val lifecycleReconciler = LifecycleReconciler(
        itemRepository, items.reconciliationSession, ProcessLifecycleOwner.get().lifecycle,
        CoroutineScope(applicationScope.coroutineContext + Dispatchers.Main.immediate),
        connectivity.state,
    )

    val pairingRepository = PairingRepository(
        credentialStore,
        lifecycleReconciler::refresh,
        DeviceIdentity(android.os.Build.MODEL, BuildConfig.VERSION_NAME, android.os.Build.VERSION.RELEASE),
    )

    internal fun authenticatedApi(): LamApi? = credentials.api()
}
