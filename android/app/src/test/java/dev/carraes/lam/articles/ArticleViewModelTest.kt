package dev.carraes.lam.articles

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.test.*
import org.junit.Assert.*
import org.junit.Test

@OptIn(kotlinx.coroutines.ExperimentalCoroutinesApi::class)
class ArticleViewModelTest {
    @Test fun `all unread escapes dates and ordinary filters restore today`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val api = FakeArticleApi()
            val repo = ArticleRepository(MemoryArticleStorage(), { api }, { ArticleSession("account", "epoch") })
            val vm = ArticleViewModel(repo)
            vm.day(articleToday().minusDays(1).toString()); runCurrent()
            assertTrue(vm.state.value.items.isEmpty())
            vm.allUnread(); runCurrent()
            assertNull(vm.state.value.day)
            assertEquals("unread", vm.state.value.readFilter)
            assertEquals(1, vm.state.value.items.size)
            vm.filter("read"); runCurrent()
            assertEquals(articleToday().toString(), vm.state.value.day)
            assertTrue(vm.state.value.items.isEmpty())
        } finally { Dispatchers.resetMain() }
    }
    @Test fun `verified visible content writes once and reload cannot undo explicit unread`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val api = FakeArticleApi()
            val session = ArticleSession("account", "epoch")
            val repo = ArticleRepository(MemoryArticleStorage(), { api }, { session })
            val vm = ArticleViewModel(repo)
            vm.open(api.article.id)
            runCurrent()
            assertTrue(api.writes.isEmpty())
            val document = vm.reader.value.document!!
            vm.visible(document)
            runCurrent()
            assertEquals(listOf(true to 0L), api.writes)
            vm.unread(api.article)
            runCurrent()
            vm.visible(document)
            vm.reload()
            runCurrent()
            vm.visible(vm.reader.value.document!!)
            runCurrent()
            assertEquals(listOf(true to 0L, false to 1L), api.writes)
            assertNull(api.article.readAt)
        } finally { Dispatchers.resetMain() }
    }

    @Test fun `failed content stays unread and a failed read keeps verified document`() = runTest {
        Dispatchers.setMain(StandardTestDispatcher(testScheduler))
        try {
            val api = FakeArticleApi().apply { failContent = true }
            val repo = ArticleRepository(MemoryArticleStorage(), { api }, { ArticleSession("account", "epoch") })
            val vm = ArticleViewModel(repo)
            vm.open(api.article.id); runCurrent()
            assertNull(vm.reader.value.document)
            assertTrue(vm.reader.value.failed)
            assertTrue(api.writes.isEmpty())
            api.failContent = false
            api.conflict = true
            vm.reload(); runCurrent()
            val document = vm.reader.value.document!!
            vm.visible(document); runCurrent()
            assertSame(document, vm.reader.value.document)
            assertEquals(ReadResult.CONFLICT, vm.reader.value.readResult)
        } finally { Dispatchers.resetMain() }
    }
}
