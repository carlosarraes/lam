package dev.carraes.lam.notifications

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ItemLinksTest {
    @Test
    fun `accepts only exact lam item view links`() {
        val links = ItemLinks()
        links.receive("android.intent.action.VIEW", "lam://items/abc29")
        assertEquals("abc29", links.pending.value)
        links.consume("abc29")
        assertNull(links.pending.value)

        for (uri in listOf(
            "lam://items/abc29/extra",
            "lam://items/ABC29",
            "lam://items/abc10",
            "lam://items/abc29?token=x",
            "https://items/abc29",
        )) {
            links.receive("android.intent.action.VIEW", uri)
            assertNull(uri, links.pending.value)
        }
        links.receive("android.intent.action.MAIN", "lam://items/abc29")
        assertNull(links.pending.value)
    }

    @Test
    fun `an old consumption cannot drop a newer notification tap`() {
        val links = ItemLinks()
        links.receive("android.intent.action.VIEW", "lam://items/abc29")
        links.receive("android.intent.action.VIEW", "lam://items/xyz89")
        links.consume("abc29")
        assertEquals("xyz89", links.pending.value)
    }
}
