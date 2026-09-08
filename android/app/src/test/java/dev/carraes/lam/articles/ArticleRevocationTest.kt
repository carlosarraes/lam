package dev.carraes.lam.articles

import dev.carraes.lam.items.*
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.test.*
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ArticleRevocationTest {
    @Test fun `every Article unauthorized path clears shared current session without foreground reconciliation`() = runTest {
        for (path in listOf("content", "list", "read", "asset", "conflict-get")) {
            val f = Fixture(backgroundScope)
            f.items.refresh()
            val doc = f.articles.load(f.api.article.id)!!
            f.articles.refresh()
            assertFalse(f.cache.rows.isEmpty())
            f.rejection = path
            when (path) {
                "content" -> assertNull(f.articles.load(doc.article.id))
                "list" -> f.articles.refresh()
                "read", "conflict-get" -> assertEquals(ReadResult.FAILED, f.articles.setRead(doc.article.id, true, 0))
                "asset" -> assertNull(f.articles.asset(doc.session, doc.article, 1, false))
            }
            assertNull(f.credentials.state.value)
            assertNull(f.binding.current())
            assertEquals(SyncState.Revoked, f.items.syncState.value)
            assertTrue(f.cache.rows.isEmpty())
            assertTrue(f.items.openItems.first().isEmpty())
            assertFalse(f.articles.isCurrent(doc.session))
            assertTrue(f.articles.state.value.items.isEmpty())
            assertNull(f.articles.load(doc.article.id))
            assertEquals(listOf("open"), f.itemApi.calls)
        }
    }

    @Test fun `late Article 401 does not revoke replacement or erase its cached content`() = runTest {
        val f = Fixture(backgroundScope)
        f.items.refresh()
        val oldSession = f.binding.current()!!
        val held = CompletableDeferred<Unit>()
        f.api.beforeContent = { held.await(); throw ArticleHttpException(401) }
        val oldLoad = async { f.articles.load(f.api.article.id) }
        runCurrent()
        f.replace()
        f.api.beforeContent = {}
        val replacement = f.articles.load(f.api.article.id)!!
        held.complete(Unit)
        assertNull(oldLoad.await())
        assertEquals(replacement.session, f.binding.current())
        assertNotEquals(oldSession.account, replacement.session.account)
        assertNotNull(f.credentials.state.value)
        assertNotNull(f.cache.get(replacement.session.account, replacement.article.id)?.content)
        assertTrue(f.articles.isCurrent(replacement.session))
    }

    @Test fun `known rejection denies epoch before suspended cleanup and conditional cleanup preserves replacement`() = runTest {
        val f = Fixture(backgroundScope)
        f.items.refresh()
        val doc = f.articles.load(f.api.article.id)!!
        val cleanup = CompletableDeferred<Unit>()
        f.beforeCleanup = { cleanup.await() }
        f.rejection = "content"
        val rejected = async { f.articles.load(doc.article.id) }
        runCurrent()
        assertFalse(f.articles.isCurrent(doc.session))
        assertNull(f.articles.load(doc.article.id))
        assertNull(f.articles.asset(doc.session, doc.article, 1, false))
        f.replace()
        f.rejection = null
        val replacement = f.articles.load(doc.article.id)!!
        cleanup.complete(Unit)
        assertNull(rejected.await())
        assertTrue(f.articles.isCurrent(replacement.session))
        assertNotNull(f.cache.get(replacement.session.account, doc.article.id)?.content)
        assertNotNull(f.credentials.state.value)
    }

    private class Fixture(scope: CoroutineScope) {
        val cache = MemoryArticleStorage()
        val credentials = FakeCredentials()
        val itemApi = FakeApi()
        private val backing = MemoryStorage()
        val items = DefaultItemRepository(object : ItemStorage by backing {
            override suspend fun clear() { backing.clear(); cache.rows.clear() }
        }, { itemApi }, credentials, scope)
        val api = FakeArticleApi()
        var rejection: String? = null
        var beforeCleanup: suspend () -> Unit = {}
        private val wire = object : ArticleApi by api {
            private fun reject(path: String) { if (rejection == path) throw ArticleHttpException(401) }
            override suspend fun list(query: String, read: String, cursor: String?): ArticlePage { reject("list"); return api.list(query, read, cursor) }
            override suspend fun content(id: String): ArticleContent { reject("content"); return api.content(id) }
            override suspend fun get(id: String): Article { reject("conflict-get"); return api.get(id) }
            override suspend fun setRead(id: String, read: Boolean, version: Long): Article {
                reject("read")
                if (rejection == "conflict-get") throw ArticleHttpException(409)
                return api.setRead(id, read, version)
            }
            override suspend fun asset(id: String, index: Int, limit: Long): ByteArray { reject("asset"); return api.asset(id, index, limit) }
        }
        val binding = ArticleSessionBinding(items.pairedSession) { wire }
        val articles = ArticleRepository(cache, binding::api, binding::current) {
            beforeCleanup()
            items.rejectCredential(it.generation)
        }
        suspend fun replace() { items.credentialStore.save(PairedServer("https://replacement.example/", "replacement", "Phone"), "replacement") }
    }
}
