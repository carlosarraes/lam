package dev.carraes.lam.articles

import kotlinx.serialization.json.*
import org.junit.Assert.*
import org.junit.Test

class ArticleResourcePolicyTest {
    private val article = articleFixture()
    private val policy = ArticleResourcePolicy("public-session-7", article)

    @Test fun `only exact article mapped inline resources may load automatically`() {
        assertTrue(policy.allowsAutomaticRequest("https://articles.lam.invalid/public-session-7/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/assets/1"))
        listOf("https://external.invalid/tracker.png", "file:///data/local/private", "content://private/1",
            "https://articles.lam.invalid/other/aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa/assets/1",
            "https://articles.lam.invalid/public-session-7/bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb/assets/1",
            policy.documentUrl, policy.assetUrl(0), policy.assetUrl(2), policy.assetUrl(1) + "?x=1",
            policy.assetUrl(1) + "#x", policy.assetUrl(1).replace("https:", "http:"),
            policy.assetUrl(1).replace("/1", "/%31"), "data:image/png;base64,AA==").forEach {
            assertFalse(it, policy.allowsAutomaticRequest(it))
        }
    }

    @Test fun `typed slots preserve ordinary text and reject attachment or malformed slots`() {
        assertEquals("<p>lam-asset:1</p><img src=\"${policy.assetUrl(1)}\">", policy.assemble(listOf(
            JsonPrimitive("<p>lam-asset:1</p><img src=\""), buildJsonObject { put("asset", 1) }, JsonPrimitive("\">"))))
        listOf(buildJsonObject { put("asset", 2) }, buildJsonObject { put("asset", -1) },
            buildJsonObject { put("asset", 1); put("extra", true) }, JsonNull, JsonPrimitive(12)).forEach {
            assertThrows(IllegalArgumentException::class.java) { policy.assemble(listOf(it)) }
        }
    }

    @Test fun `safe outbound URLs exclude ambiguous and privileged destinations`() {
        assertTrue(ArticleResourcePolicy.safeExternal("https://example.org/report?q=1#part"))
        listOf("javascript:alert(1)", "intent://app", "file:///private", "https://user:pass@example.org/",
            "https://example.org/\n", "https://example.org\\evil", "//example.org").forEach {
            assertFalse(it, ArticleResourcePolicy.safeExternal(it))
        }
    }
}

internal fun articleFixture(id: String = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", version: Long = 0, read: Boolean = false) = Article(
    id, "Release notes", "Readable summary", "Agent", "host", "lam", "${articleToday()}T12:00:00Z",
    if (read) "2026-09-08T12:01:00Z" else null, version,
    listOf(ArticleAsset("index.html", "text/html", 20, "0".repeat(64), "inline"),
        ArticleAsset("chart.png", "image/png", 4, sha256(byteArrayOf(1, 2, 3, 4)), "inline"),
        ArticleAsset("notes.txt", "text/plain", 4, sha256("note".toByteArray()), "attachment")),
)
