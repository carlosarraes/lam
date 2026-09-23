package dev.carraes.lam.notifications

import androidx.lifecycle.ViewModel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

class ItemLinks : ViewModel() {
    private val value = MutableStateFlow<String?>(null)
    val pending = value.asStateFlow()

    fun receive(action: String?, uri: String?) {
        if (action != "android.intent.action.VIEW" || uri == null) return
        val id = Regex("lam://items/([a-z2-9]{5})").matchEntire(uri)?.groupValues?.get(1) ?: return
        value.value = id
    }

    fun consume(id: String) {
        value.compareAndSet(id, null)
    }
}
