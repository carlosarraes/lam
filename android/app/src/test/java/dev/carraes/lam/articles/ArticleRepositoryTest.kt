package dev.carraes.lam.articles

import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ArticleRepositoryTest {
    @Test fun `cancelled cache lookup propagates cancellation instead of publishing a failed old load`() = runTest {
        val api = FakeArticleApi().apply { failContent = true }
        val storage = object : ArticleStorage {
            override suspend fun list(account: String) = emptyList<ArticleEntity>()
            override suspend fun get(account: String, id: String): ArticleEntity? = throw CancellationException("reader closed")
            override suspend fun save(rows: List<ArticleEntity>, current: () -> Boolean) = false
        }
        val repo = ArticleRepository(storage, { api }, { ArticleSession("account", "epoch") })
        try { repo.load(api.article.id); fail("Cancellation was swallowed") } catch (_: CancellationException) { }
    }

    @Test fun `list pages deduplicate and older responses cannot undo read state`() = runTest {
        val f = Fixture()
        f.api.page = ArticlePage(listOf(articleFixture()), "page2")
        f.repo.refresh()
        assertEquals("page2", f.repo.state.value.nextCursor)
        assertEquals(ReadResult.SAVED, f.repo.setRead(f.api.article.id, true, 0))
        f.api.page = ArticlePage(listOf(articleFixture(), articleFixture("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")), null)
        f.repo.loadMore()
        assertEquals(2, f.repo.state.value.items.size)
        assertEquals(1L, f.repo.state.value.items.single { it.id == f.api.article.id }.version)
        assertNotNull(f.repo.state.value.items.single { it.id == f.api.article.id }.readAt)
    }

    @Test fun `read conflict fetches canonical state once and never replays mutation`() = runTest {
        val f = Fixture()
        f.repo.refresh()
        f.api.article = articleFixture(version = 2)
        f.api.conflict = true
        assertEquals(ReadResult.CONFLICT, f.repo.setRead(f.api.article.id, true, 0))
        assertEquals(2L, f.repo.state.value.items.single().version)
        assertNull(f.repo.state.value.items.single().readAt)
        assertEquals(listOf(true to 0L), f.api.writes)
    }

    @Test fun `content loads do not mark read and original manifest digest is separate from cache integrity`() = runTest {
        val f = Fixture()
        val loaded = f.repo.load(f.api.article.id)!!
        assertEquals("<p>Readable</p>", loaded.html)
        assertTrue(f.api.writes.isEmpty())
        f.api.failContent = true
        val cached = f.repo.load(f.api.article.id)!!
        assertTrue(cached.cached)
        assertEquals(loaded.html, cached.html)
        f.storage.rows.values.single().let { f.storage.rows[it.account to it.id] = it.copy(content = "tampered") }
        assertNull(f.repo.load(f.api.article.id))
        assertTrue(f.api.writes.isEmpty())
    }

    @Test fun `account switch discards pending document and all old resource authority`() = runTest {
        val f = Fixture()
        val held = CompletableDeferred<Unit>()
        f.api.beforeContent = { held.await() }
        val opening = async { f.repo.load(f.api.article.id) }
        runCurrent()
        f.session.value = ArticleSession("account-b", "epoch-b")
        held.complete(Unit)
        assertNull(opening.await())
        assertTrue(f.storage.rows.isEmpty())
        assertNull(f.repo.asset(ArticleSession("account-a", "epoch-a"), f.api.article, 1, false))
    }

    @Test fun `obsolete query cannot replace the active search and read filter`() = runTest {
        val f = Fixture()
        val held = CompletableDeferred<Unit>()
        f.api.beforeList = { if (it == "old") held.await() }
        val old = async { f.repo.refresh("old", "all") }
        runCurrent()
        f.api.page = ArticlePage(emptyList(), null)
        f.repo.refresh("new", "unread")
        held.complete(Unit)
        old.await()
        assertEquals("new", f.repo.state.value.query)
        assertEquals("unread", f.repo.state.value.readFilter)
        assertTrue(f.repo.state.value.items.isEmpty())
    }

    @Test fun `attachment requires explicit operation and resource integrity is checked`() = runTest {
        val f = Fixture()
        assertNull(f.repo.asset(f.session.value!!, f.api.article, 2, false))
        assertArrayEquals("note".toByteArray(), f.repo.asset(f.session.value!!, f.api.article, 2, true))
        f.api.bytes = byteArrayOf(9, 9, 9, 9)
        assertNull(f.repo.asset(f.session.value!!, f.api.article, 1, false))
    }

    private class Fixture {
        val session = MutableStateFlow<ArticleSession?>(ArticleSession("account-a", "epoch-a"))
        val storage = MemoryArticleStorage()
        val api = FakeArticleApi()
        val repo = ArticleRepository(storage, { api }, { session.value })
    }
}

internal class MemoryArticleStorage : ArticleStorage {
    val rows = mutableMapOf<Pair<String, String>, ArticleEntity>()
    override suspend fun list(account: String) = rows.values.filter { it.account == account }
    override suspend fun get(account: String, id: String) = rows[account to id]
    override suspend fun save(rows: List<ArticleEntity>, current: () -> Boolean): Boolean {
        if (!current()) return false
        rows.forEach { row ->
            val old = this.rows[row.account to row.id]
            val canonical = if (old != null && old.article().version > row.article().version) old.json else row.json
            this.rows[row.account to row.id] = row.copy(json = canonical, content = row.content ?: old?.content,
                contentSha256 = row.contentSha256 ?: old?.contentSha256)
        }
        return true
    }
}

internal class FakeArticleApi : ArticleApi {
    var article = articleFixture()
    var page = ArticlePage(listOf(article), null)
    var conflict = false
    var failContent = false
    var bytes = byteArrayOf(1, 2, 3, 4)
    var beforeContent: suspend () -> Unit = {}
    var beforeList: suspend (String) -> Unit = {}
    val writes = mutableListOf<Pair<Boolean, Long>>()
    override suspend fun list(query: String, read: String, cursor: String?): ArticlePage { beforeList(query); return page }
    override suspend fun get(id: String) = article
    override suspend fun content(id: String): ArticleContent {
        beforeContent()
        if (failContent) throw java.io.IOException()
        return ArticleContent(article, listOf(JsonPrimitive("<p>Readable</p>")))
    }
    override suspend fun setRead(id: String, read: Boolean, version: Long): Article {
        writes += read to version
        if (conflict) throw ArticleHttpException(409)
        article = articleFixture(version = version + 1, read = read)
        return article
    }
    override suspend fun asset(id: String, index: Int, limit: Long) = if (index == 2) "note".toByteArray() else bytes
}
