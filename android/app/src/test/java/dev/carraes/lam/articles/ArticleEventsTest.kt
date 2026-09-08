package dev.carraes.lam.articles

import kotlinx.coroutines.*
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.collect
import kotlinx.serialization.encodeToString
import okhttp3.*
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

class ArticleEventsTest {
    @Test fun `real websocket reconnect reloads canonical state and article frames never cause read writes`() = runBlocking {
        MockWebServer().use { server ->
            val version = AtomicLong(0)
            val sockets = LinkedBlockingQueue<WebSocket>()
            val requests = LinkedBlockingQueue<RecordedRequest>()
            server.dispatcher = object : okhttp3.mockwebserver.Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse {
                    requests.add(request)
                    return if (request.path == "/v2/events") MockResponse().withWebSocketUpgrade(object : WebSocketListener() {
                        override fun onOpen(webSocket: WebSocket, response: Response) { sockets.add(webSocket) }
                    }) else MockResponse().setBody(articleJson.encodeToString(ArticlePage(listOf(articleFixture(version = version.get(), read = version.get() > 0)), null)))
                }
            }
            val api = OkHttpArticleApi(server.url("/"), { "event-test-credential" })
            val repo = ArticleRepository(MemoryArticleStorage(), { api }, { ArticleSession("account", "epoch") })
            val syncing = launch(Dispatchers.IO) { repo.reconcileForeground(0) }
            try {
                val first = withContext(Dispatchers.IO) { sockets.poll(5, TimeUnit.SECONDS) }!!
                withTimeout(5_000) { while (repo.state.value.loading || repo.state.value.items.isEmpty()) delay(10) }
                version.set(1)
                first.send("""{"event":"article.read_changed","article_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","version":1}""")
                withTimeout(5_000) { while (repo.state.value.items.single().version != 1L) delay(10) }
                first.close(1000, null)
                version.set(2)
                val second = withContext(Dispatchers.IO) { sockets.poll(8, TimeUnit.SECONDS) }
                assertNotNull(second)
                withTimeout(5_000) { while (repo.state.value.items.single().version != 2L) delay(10) }
                assertTrue(requests.all { it.method == "GET" && it.getHeader("Authorization") == "Bearer event-test-credential" })
                assertEquals(2, requests.count { it.path == "/v2/events" })
            } finally { syncing.cancelAndJoin() }
        }
    }

    @Test fun `event parser accepts only article identity and numeric version without excess payload`() {
        val frame = """{"event":"article.published","article_id":"aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa","version":0}"""
        assertTrue(isArticleInvalidation(frame))
        for (bad in listOf(frame.replace(":0", ":-1"), frame.replace(":0", ":\"1\""), frame.replace("published", "deleted"),
            frame.dropLast(1) + ",\"html\":\"private\"}", frame.replace("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "../other"))) {
            assertFalse(isArticleInvalidation(bad))
        }
    }

    @Test fun `stream 401 uses shared conditional revocation and stale stream cannot reject replacement`() = runBlocking {
        for (replace in listOf(false, true)) {
            var session = ArticleSession("account", "epoch", 1)
            val entered = CompletableDeferred<Unit>(); val release = CompletableDeferred<Unit>()
            val api = FakeArticleApi().apply { stream = kotlinx.coroutines.flow.flow {
                entered.complete(Unit); release.await(); throw ArticleHttpException(401)
            } }
            val rejected = mutableListOf<ArticleSession>()
            val repo = ArticleRepository(MemoryArticleStorage(), { api }, { session }, { rejected += it })
            val syncing = launch { repo.reconcileForeground(1) }
            entered.await()
            if (replace) session = ArticleSession("replacement", "new", 2)
            release.complete(Unit); syncing.join()
            assertEquals(if (replace) 0 else 1, rejected.size)
            assertEquals(replace, repo.isCurrent(session))
        }
    }
}
