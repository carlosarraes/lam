package dev.carraes.lam.ui.detail

import dev.carraes.lam.ui.markdown.isSupportedWebLink
import org.junit.Assert.*
import org.junit.Test

class MarkdownLinkTest {
    @Test fun onlyAbsoluteHttpDestinationsWithoutCredentialsOrControlCharactersMayOpen() {
        listOf("https://example.com/path?q=1#section", "http://localhost:8080", "HTTPS://example.com").forEach {
            assertTrue(it, isSupportedWebLink(it))
        }
        listOf("javascript:alert(1)", "file:///etc/passwd", "intent://host/#Intent;end", "data:text/html,hello",
            "mailto:user@example.com", "//example.com", "/relative", "https:", "https:///no-host", "https://user@example.com",
            "https://example.com\n", "https://example.com\\@evil.com").forEach {
            assertFalse(it, isSupportedWebLink(it))
        }
    }
}
