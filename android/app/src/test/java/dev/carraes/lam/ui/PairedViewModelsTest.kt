package dev.carraes.lam.ui

import androidx.lifecycle.ViewModel
import org.junit.Assert.*
import org.junit.Test

class PairedViewModelsTest {
    @Test fun `same session retains detail lifecycle while unpair or replacement clears it`() {
        val owner = PairedViewModels()
        owner.bind(1)
        val detail = TrackedViewModel()
        owner.viewModelStore.put("detail", detail)
        owner.bind(1)
        assertSame(detail, owner.viewModelStore.get("detail"))
        assertFalse(detail.cleared)
        owner.bind(null)
        assertTrue(detail.cleared)
        assertNull(owner.viewModelStore.get("detail"))
        owner.bind(2)
        assertNull(owner.viewModelStore.get("detail"))
    }
    private class TrackedViewModel : ViewModel() {
        var cleared = false
        override fun onCleared() { cleared = true }
    }
}
