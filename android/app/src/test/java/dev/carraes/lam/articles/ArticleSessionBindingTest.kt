package dev.carraes.lam.articles

import dev.carraes.lam.items.PairedSession
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.*
import org.junit.Test

class ArticleSessionBindingTest {
    @Test fun `snapshot replacement changes epoch and denies API captured across credential transition`() {
        val server = PairedServer("https://example.com/", "device", "Phone")
        val snapshots = MutableStateFlow<PairedSession?>(PairedSession(1, server))
        var replaceDuringFactory = false
        val binding = ArticleSessionBinding(snapshots) {
            if (replaceDuringFactory) snapshots.value = PairedSession(2, server)
            FakeArticleApi()
        }
        val old = binding.current()!!
        assertSame(old, binding.current())
        assertNotNull(binding.api(old))
        replaceDuringFactory = true
        assertNull(binding.api(old))
        val current = binding.current()!!
        assertEquals(old.account, current.account)
        assertNotEquals(old.epoch, current.epoch)
        assertEquals(2L, current.generation)
        snapshots.value = null
        assertNull(binding.current())
        assertNull(binding.api(current))
    }
}
