package dev.carraes.lam.ui.detail

import dev.carraes.lam.ui.markdown.isSupportedLink
import org.junit.Assert.*
import org.junit.Test

class MarkdownLinkTest {
    @Test fun grokbotCredentialsAreRejectedEvenWithAnAppAuthorityThatIsNotAWebHost() {
        assertFalse(isSupportedLink("grokbot://user@app_name/v1/agent"))
    }

    @Test fun grokbotDestinationsUseTheSameSchemeAllowListAsItemLinks() {
        listOf("grokbot://app/v1/agent?id=agent-123", "GROKBOT://app/v1/agent?id=agent-123", "gRoKbOt://app").forEach {
            assertTrue(it, isSupportedLink(it))
        }
        listOf("ftp://x", "grokbot:", "grokbot://", "grokbot:app", "grokbot://app with spaces",
            "grokbot://user@app", "grokbot://app\n", "grokbot://app\\@evil.com", "bare text").forEach {
            assertFalse(it, isSupportedLink(it))
        }
    }

    @Test fun onlyAbsoluteHttpDestinationsWithoutCredentialsOrControlCharactersMayOpen() {
        listOf("https://example.com/path?q=1#section", "http://localhost:8080", "HTTPS://example.com").forEach {
            assertTrue(it, isSupportedLink(it))
        }
        listOf("javascript:alert(1)", "file:///etc/passwd", "intent://host/#Intent;end", "data:text/html,hello",
            "mailto:user@example.com", "//example.com", "/relative", "https:", "https:///no-host", "https://user@example.com",
            "https://example.com\n", "https://example.com\\@evil.com").forEach {
            assertFalse(it, isSupportedLink(it))
        }
    }
}
