package dev.carraes.lam.articles

import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.withContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.serialization.encodeToString
import java.util.concurrent.atomic.AtomicLong
import java.util.concurrent.ConcurrentHashMap

data class ArticlesState(val items: List<Article> = emptyList(), val query: String = "", val readFilter: String = "all",
    val nextCursor: String? = null, val loading: Boolean = false, val failed: Boolean = false, val cached: Boolean = false,
    val day: String? = articleToday().toString())

class ArticleRepository(private val storage: ArticleStorage, private val api: (ArticleSession) -> ArticleApi?,
    private val session: () -> ArticleSession?, private val onUnauthorized: suspend (ArticleSession) -> Unit = {}) {
    private val mutableState = MutableStateFlow(ArticlesState())
    val state = mutableState.asStateFlow()
    private val generation = AtomicLong()
    private val rejected = ConcurrentHashMap.newKeySet<ArticleSession>()
    private var boundSession: ArticleSession? = null
    fun isCurrent(captured: ArticleSession) = session() == captured && captured !in rejected
    @Synchronized fun reset() { boundSession = session(); generation.incrementAndGet(); mutableState.value = ArticlesState() }
    @Synchronized fun sessionChanged(): ArticleSession? {
        val current = session()
        if (boundSession != current) { boundSession = current; generation.incrementAndGet(); mutableState.value = ArticlesState() }
        return current?.takeIf(::isCurrent)
    }
    suspend fun reconcileForeground(expectedGeneration: Long) {
        val captured = sessionChanged()?.takeIf { it.generation == expectedGeneration } ?: return
        refresh()
        while (isCurrent(captured)) {
            try {
                val stream = api(captured) ?: return
                // Open/reconnect and invalidations both trigger canonical GETs. Conflation retains
                // one pending invalidation while a GET is running, including a newer read change.
                stream.events().collect { if (isCurrent(captured)) refresh() }
            } catch (cancelled: CancellationException) { throw cancelled }
            catch (error: Exception) { if (rejectUnauthorized(captured, error)) return }
            delay(5_000)
        }
    }

    suspend fun refresh(query: String? = null, read: String? = null, day: String? = state.value.day) {
        val captured = sessionChanged() ?: return reset()
        val requestedQuery = query ?: state.value.query
        val requestedRead = read ?: state.value.readFilter
        require(requestedRead in setOf("all", "read", "unread"))
        day?.let { require(!java.time.LocalDate.parse(it).isAfter(articleToday())) }
        val ticket = generation.incrementAndGet()
        val sameQuery = state.value.query == requestedQuery && state.value.readFilter == requestedRead && state.value.day == day
        mutableState.value = if (sameQuery) state.value.copy(loading = true, failed = false)
            else ArticlesState(query = requestedQuery, readFilter = requestedRead, loading = true, day = day)
        fetchPage(captured, ticket, null)
    }

    suspend fun loadMore() {
        val captured = sessionChanged() ?: return
        val page = state.value
        if (page.loading || page.nextCursor == null) return
        mutableState.update { it.copy(loading = true, failed = false) }
        fetchPage(captured, generation.get(), page.nextCursor)
    }

    private suspend fun fetchPage(captured: ArticleSession, ticket: Long, cursor: String?) {
        val requested = state.value
        fun current() = isCurrent(captured) && generation.get() == ticket
        try {
            val page = requireNotNull(api(captured)).list(requested.query, requested.readFilter, cursor, requested.day)
            require(page.items.size <= 25)
            val rows = page.items.map { validateArticle(it); ArticleEntity.from(captured.account, it) }
            if (!storage.save(rows, ::current) || !current()) return
            val canonical = page.items.map { storage.get(captured.account, it.id)!!.article() }
            if (!current()) return
            mutableState.update { old -> old.copy(items = filter((if (cursor == null) canonical else old.items + canonical)
                .associateBy { it.id }.values.toList(), old), nextCursor = page.nextCursor, loading = false, failed = false, cached = false) }
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) {
            if (rejectUnauthorized(captured, error)) return
            if (!current()) return
            val saved = runCatching { storage.list(captured.account).map { it.article() } }.getOrDefault(emptyList())
            if (current()) mutableState.update { it.copy(items = if (it.items.isEmpty()) filter(saved, it) else it.items,
                loading = false, failed = true, cached = true) }
        }
    }

    suspend fun load(id: String): LoadedArticle? {
        val captured = sessionChanged() ?: return null
        try {
            val content = requireNotNull(api(captured)).content(id)
            validateArticle(content.article)
            require(content.article.id == id)
            val html = ArticleResourcePolicy(captured.epoch, content.article).assemble(content.parts)
            // Cache typed parts, so process/session changes rebuild URLs using the current nonsecret epoch.
            val serialized = articleJson.encodeToString(content)
            if (!storage.save(listOf(ArticleEntity.from(captured.account, content.article, serialized)), { isCurrent(captured) })) return null
            return if (isCurrent(captured)) LoadedArticle(content.article, html, captured) else null
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) {
            if (rejectUnauthorized(captured, error)) return null
            if (!isCurrent(captured)) return null
            return try {
                val row = storage.get(captured.account, id) ?: return null
                val bytes = row.content ?: return null
                require(bytes.toByteArray().size <= 16 * 1024 * 1024 && sha256(bytes.toByteArray()) == row.contentSha256)
                val cached = articleJson.decodeFromString<ArticleContent>(bytes)
                require(cached.article.id == id)
                validateArticle(cached.article)
                val html = ArticleResourcePolicy(captured.epoch, cached.article).assemble(cached.parts)
                if (isCurrent(captured)) LoadedArticle(cached.article, html, captured, cached = true) else null
            } catch (cancelled: CancellationException) { throw cancelled } catch (_: Exception) { null }
        }
    }

    suspend fun setRead(id: String, read: Boolean, version: Long, captured: ArticleSession? = session()): ReadResult {
        if (captured == null || !isCurrent(captured)) return ReadResult.FAILED
        val client = api(captured) ?: return ReadResult.FAILED
        return try {
            val canonical = client.setRead(id, read, version)
            if (accept(captured, id, canonical)) ReadResult.SAVED else ReadResult.FAILED
        } catch (cancelled: CancellationException) { throw cancelled }
        catch (error: Exception) {
            if (rejectUnauthorized(captured, error)) return ReadResult.FAILED
            if (error is ArticleHttpException && error.status == 409) {
                try { accept(captured, id, client.get(id)) } catch (cancelled: CancellationException) { throw cancelled } catch (recoveryError: Exception) {
                    if (rejectUnauthorized(captured, recoveryError)) return ReadResult.FAILED
                }
                ReadResult.CONFLICT
            } else ReadResult.FAILED
        }
    }

    private suspend fun accept(captured: ArticleSession, id: String, article: Article): Boolean {
        validateArticle(article); require(article.id == id)
        if (!storage.save(listOf(ArticleEntity.from(captured.account, article)), { isCurrent(captured) }) || !isCurrent(captured)) return false
        val canonical = storage.get(captured.account, id)!!.article()
        if (!isCurrent(captured)) return false
        mutableState.update { it.copy(items = filter(it.items.map { old -> if (old.id == id) canonical else old }, it)) }
        return true
    }

    suspend fun asset(captured: ArticleSession, article: Article, index: Int, attachment: Boolean): ByteArray? {
        if (!isCurrent(captured)) return null
        val asset = article.assets.getOrNull(index) ?: return null
        if (if (attachment) asset.disposition != "attachment" else !asset.inlineImage) return null
        val minimumSize = if (attachment) 0L else 1L
        if (asset.size !in minimumSize..20L * 1024 * 1024) return null
        return try {
            val bytes = requireNotNull(api(captured)).asset(article.id, index, asset.size)
            if (isCurrent(captured) && bytes.size.toLong() == asset.size && sha256(bytes) == asset.sha256) bytes else null
        } catch (cancelled: CancellationException) { throw cancelled } catch (error: Exception) {
            rejectUnauthorized(captured, error)
            null
        }
    }

    private suspend fun rejectUnauthorized(captured: ArticleSession, error: Exception): Boolean {
        if (error !is ArticleHttpException || error.status != 401) return false
        if (session() == captured) {
            // Deny cached content/resources before shared cleanup can suspend or fail.
            rejected.add(captured)
            if (session() == captured) reset()
            withContext(NonCancellable) { onUnauthorized(captured) }
        }
        return true
    }

    private fun filter(articles: List<Article>, state: ArticlesState) = articles.filter {
        (state.day == null || articleOnDay(it.createdAt, state.day)) &&
            (state.readFilter == "all" || (it.readAt != null) == (state.readFilter == "read")) &&
            (state.query.isBlank() || listOf(it.title, it.summary, it.name).any { value -> value.contains(state.query, ignoreCase = true) })
    }.sortedWith(compareByDescending<Article> { it.createdAt }.thenByDescending { it.id })
}

internal fun validateArticle(article: Article) {
    require(article.id.matches(Regex("[a-f0-9-]{36}")) && article.version >= 0)
    require(article.assets.size in 1..51 && article.assets.count { it.path == "index.html" } == 1)
    require(article.assets.all { it.size in 0..20L * 1024 * 1024 && it.sha256.matches(Regex("[a-f0-9]{64}")) })
    require(article.assets.none { it.inlineImage && it.size == 0L })
    require(article.assets.sumOf { it.size } <= 50L * 1024 * 1024)
}
