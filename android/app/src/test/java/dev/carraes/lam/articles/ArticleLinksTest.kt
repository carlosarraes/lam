package dev.carraes.lam.articles

import org.junit.Assert.*
import org.junit.Test

class ArticleLinksTest {
    @Test fun `stable ID links reject aliases capabilities and unintended actions`() {
        val id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        val route = ArticleLinks()
        route.receive("android.intent.action.VIEW", "lam://articles/$id")
        assertEquals(id, route.pending.value)
        route.consume(id); assertNull(route.pending.value)
        for (value in listOf("lam://user@articles/$id", "lam://articles:80/$id", "lam://articles/$id/extra", "lam://articles/$id?token=x",
            "lam://articles/$id#bootstrap", "lam://articles/AAAAAAAA-AAAA-AAAA-AAAA-AAAAAAAAAAAA", "lam://articles/$id/", "https://articles/$id", "lam://articles/1-1-1-1-1")) {
            route.receive("android.intent.action.VIEW", value); assertNull(value, route.pending.value)
        }
        route.receive("android.intent.action.MAIN", "lam://articles/$id"); assertNull(route.pending.value)
    }
    @Test fun `pending link survives unpaired wait and old consumption cannot drop newer intent`() {
        val route = ArticleLinks()
        val first = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        val second = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        route.receive("android.intent.action.VIEW", "lam://articles/$first")
        assertEquals(first, route.pending.value)
        route.receive("android.intent.action.VIEW", "lam://articles/$second")
        route.consume(first); assertEquals(second, route.pending.value)
        route.consume(second); route.consume(second); assertNull(route.pending.value)
    }
}
