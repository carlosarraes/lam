package dev.carraes.lam.articles

import androidx.activity.ComponentActivity
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import dev.carraes.lam.ui.PairedViewModels
import org.junit.Assert.*
import org.junit.Test

class ArticleRotationTest {
    @Test fun activityRecreationRetainsReaderAndSameSessionButUnpairClearsIt() {
        ActivityScenario.launch(ComponentActivity::class.java).use { scenario ->
            lateinit var original: PairedViewModels
            val reader = ReaderLifetime()
            scenario.onActivity { activity ->
                original = ViewModelProvider(activity)[PairedViewModels::class.java]
                original.bind(7)
                original.viewModelStore.put("reader", reader)
            }
            scenario.recreate()
            scenario.onActivity { activity ->
                val restored = ViewModelProvider(activity)[PairedViewModels::class.java]
                restored.bind(7)
                assertSame(original, restored)
                assertSame(reader, restored.viewModelStore.get("reader"))
                assertFalse(reader.cleared)
                restored.bind(null)
                assertTrue(reader.cleared)
                assertNull(restored.viewModelStore.get("reader"))
            }
        }
    }
    private class ReaderLifetime : ViewModel() {
        var cleared = false
        override fun onCleared() { cleared = true }
    }
}
