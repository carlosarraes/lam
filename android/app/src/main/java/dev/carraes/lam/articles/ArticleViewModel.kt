package dev.carraes.lam.articles

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class ArticleReaderState(val id: String? = null, val document: LoadedArticle? = null, val loading: Boolean = false,
    val failed: Boolean = false, val readResult: ReadResult? = null, val renderGeneration: Int = 0)

class ArticleViewModel(val repository: ArticleRepository) : ViewModel() {
    val state = repository.state
    private val mutableReader = MutableStateFlow(ArticleReaderState())
    val reader = mutableReader.asStateFlow()
    private var opening: Job? = null
    private var attempted = false
    private val mutableFeedback = MutableStateFlow<ReadResult?>(null)
    val feedback = mutableFeedback.asStateFlow()

    fun refresh() { viewModelScope.launch { repository.refresh() } }
    fun query(query: String) { viewModelScope.launch { repository.refresh(query = query) } }
    fun filter(read: String) { viewModelScope.launch { repository.refresh(read = read) } }
    fun more() { viewModelScope.launch { repository.loadMore() } }
    fun open(id: String) {
        opening?.cancel()
        attempted = false
        mutableReader.value = ArticleReaderState(id = id, loading = true)
        opening = viewModelScope.launch {
            val loaded = repository.load(id)
            mutableReader.update { it.copy(document = loaded, loading = false, failed = loaded == null) }
        }
    }
    fun close() { opening?.cancel(); mutableReader.value = ArticleReaderState(); attempted = true }
    fun reload() {
        val current = reader.value
        if (current.document == null) current.id?.let(::open)
        else mutableReader.update { it.copy(renderGeneration = it.renderGeneration + 1) }
    }
    fun visible(document: LoadedArticle) {
        if (attempted || reader.value.document !== document || !repository.isCurrent(document.session)) return
        attempted = true
        viewModelScope.launch {
            val result = repository.setRead(document.article.id, true, document.article.version, document.session)
            if (reader.value.document === document) mutableReader.update { it.copy(readResult = result) }
        }
    }
    fun unread(article: Article) {
        if (reader.value.id == article.id) attempted = true
        viewModelScope.launch {
            val result = repository.setRead(article.id, false, article.version)
            mutableFeedback.value = result
            if (reader.value.id == article.id) mutableReader.update { it.copy(readResult = null) }
        }
    }
    fun consumeFeedback() { mutableFeedback.value = null }
}
