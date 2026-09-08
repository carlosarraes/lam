package dev.carraes.lam.articles

import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.test.*
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.*
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ArticleRepositoryTest {
    @Test fun `delayed session collector cannot reset a refresh started by synchronous binding`() = runTest {
        val f = Fixture()
        f.repo.refresh()
        f.session.value = ArticleSession("new-account", "new-epoch", 2)
        val held = CompletableDeferred<Unit>()
        f.api.beforeList = { held.await() }
        val refreshing = async { f.repo.refresh() }
        runCurrent()
        f.repo.sessionChanged()
        held.complete(Unit); refreshing.await()
        assertEquals(listOf(f.api.article.id), f.repo.state.value.items.map { it.id })
        assertFalse(f.repo.state.value.loading)
    }

    @Test fun `foreground invalidation during held refresh queues another canonical load`() = runTest {
        val f = Fixture()
        val events = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
        f.api.stream = events.receiveAsFlow()
        val syncing = backgroundScope.launch { f.repo.reconcileForeground(0) }
        runCurrent()
        val held = CompletableDeferred<Unit>()
        f.api.beforeList = { held.await() }
        val callsBefore = f.api.listCalls
        events.send(Unit); runCurrent()
        assertEquals(callsBefore + 1, f.api.listCalls)
        f.api.page = ArticlePage(listOf(articleFixture(version = 3, read = true)), null)
        events.send(Unit)
        held.complete(Unit); runCurrent()
        assertEquals("The held response captured version 0, so a second GET must consume the invalidation", callsBefore + 2, f.api.listCalls)
        assertEquals(listOf(0L, 3L), f.api.returnedVersions.takeLast(2))
        assertEquals(3L, f.repo.state.value.items.single().version)
        assertNotNull(f.repo.state.value.items.single().readAt)
        assertTrue(f.api.writes.isEmpty())
        syncing.cancel()
    }
    @Test fun `explicit content 401 denies cached HTML without a lifecycle or account transition`() = runTest {
        val f = Fixture()
        val loaded = f.repo.load(f.api.article.id)!!
        f.repo.refresh()
        f.api.beforeContent = { throw ArticleHttpException(401) }
        assertNull(f.repo.load(f.api.article.id))
        assertFalse(f.repo.isCurrent(loaded.session))
        assertTrue(f.repo.state.value.items.isEmpty())
        assertNull(f.repo.load(f.api.article.id))
    }

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

    @Test fun `mixed page content and explicit download accept an empty text attachment`() = runTest {
        val emptySha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        val emptyAttachment = articleFixture("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb").copy(assets = listOf(
            ArticleAsset("index.html", "text/html", 20, "0".repeat(64), "inline"),
            ArticleAsset("notes.txt", "text/plain", 0, emptySha256, "attachment"),
        ))
        val f = Fixture()
        f.api.article = emptyAttachment
        f.api.page = ArticlePage(listOf(articleFixture(), emptyAttachment), null)
        f.api.bytes = ByteArray(0)

        f.repo.refresh()

        assertFalse(f.repo.state.value.failed)
        assertEquals(setOf(articleFixture().id, emptyAttachment.id), f.repo.state.value.items.map { it.id }.toSet())
        val loaded = f.repo.load(emptyAttachment.id)
        assertNotNull(loaded)
        assertEquals(emptySha256, loaded!!.article.assets[1].sha256)
        assertArrayEquals(ByteArray(0), f.repo.asset(loaded.session, loaded.article, 1, attachment = true))
        assertEquals(listOf(Triple(emptyAttachment.id, 1, 0L)), f.api.assetRequests)
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
    var stream: kotlinx.coroutines.flow.Flow<Unit> = kotlinx.coroutines.flow.emptyFlow()
    override fun events() = stream
    var article = articleFixture()
    var page = ArticlePage(listOf(article), null)
    var conflict = false
    var failContent = false
    var bytes = byteArrayOf(1, 2, 3, 4)
    var beforeContent: suspend () -> Unit = {}
    var beforeList: suspend (String) -> Unit = {}
    var listCalls = 0
    val returnedVersions = mutableListOf<Long>()
    val writes = mutableListOf<Pair<Boolean, Long>>()
    val assetRequests = mutableListOf<Triple<String, Int, Long>>()
    override suspend fun list(query: String, read: String, cursor: String?): ArticlePage {
        val response = page
        listCalls++
        beforeList(query)
        response.items.firstOrNull()?.let { returnedVersions += it.version }
        return response
    }
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
    override suspend fun asset(id: String, index: Int, limit: Long): ByteArray {
        assetRequests += Triple(id, index, limit)
        return if (index == 2) "note".toByteArray() else bytes
    }
}
