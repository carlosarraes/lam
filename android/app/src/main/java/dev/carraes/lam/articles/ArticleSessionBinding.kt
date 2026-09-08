package dev.carraes.lam.articles

import dev.carraes.lam.items.PairedSession
import kotlinx.coroutines.flow.StateFlow
import java.util.UUID

/** Pairing metadata and generation come from one publication, never independent flow replays. */
internal class ArticleSessionBinding(private val snapshots: StateFlow<PairedSession?>, private val createApi: () -> ArticleApi?) {
    private var bound: Pair<PairedSession, ArticleSession>? = null

    @Synchronized fun current(): ArticleSession? {
        val snapshot = snapshots.value ?: run { bound = null; return null }
        if (bound?.first != snapshot) {
            val server = snapshot.server
            bound = snapshot to ArticleSession(sha256("${server.serverUrl}\n${server.deviceId}".toByteArray()), UUID.randomUUID().toString(), snapshot.generation)
        }
        return bound!!.second
    }

    fun api(expected: ArticleSession): ArticleApi? {
        if (current() != expected) return null
        val api = createApi()
        // Credential replacement invalidates the snapshot before changing the native provider.
        return api.takeIf { current() == expected }
    }
}
