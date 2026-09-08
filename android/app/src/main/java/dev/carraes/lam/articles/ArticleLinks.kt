package dev.carraes.lam.articles

import androidx.lifecycle.ViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

class ArticleLinks : ViewModel() {
    private val value = MutableStateFlow<String?>(null)
    val pending = value.asStateFlow()
    fun receive(action: String?, uri: String?) {
        if (action != "android.intent.action.VIEW" || uri == null) return
        val id = Regex("lam://articles/([a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12})").matchEntire(uri)?.groupValues?.get(1) ?: return
        value.value = id
    }
    fun consume(id: String) { value.compareAndSet(id, null) }
}
