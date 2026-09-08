package dev.carraes.lam.articles

import androidx.lifecycle.ViewModel
import dev.carraes.lam.ui.PairedViewModels
import org.junit.Assert.*
import org.junit.Test

class PairedViewModelsTest {
    @Test fun `same session retains reader lifecycle while unpair or replacement clears it`() {
        val owner = PairedViewModels()
        owner.bind(1)
        val reader = TrackedViewModel()
        owner.viewModelStore.put("article", reader)
        owner.bind(1)
        assertSame(reader, owner.viewModelStore.get("article"))
        assertFalse(reader.cleared)
        owner.bind(null)
        assertTrue(reader.cleared)
        assertNull(owner.viewModelStore.get("article"))
        owner.bind(2)
        assertNull(owner.viewModelStore.get("article"))
    }
    private class TrackedViewModel : ViewModel() {
        var cleared = false
        override fun onCleared() { cleared = true }
    }
}
