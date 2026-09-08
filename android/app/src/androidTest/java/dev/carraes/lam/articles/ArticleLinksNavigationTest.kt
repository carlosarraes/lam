package dev.carraes.lam.articles

import android.content.Intent
import android.net.Uri
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import android.view.accessibility.AccessibilityNodeInfo
import android.os.SystemClock
import dev.carraes.lam.LamApplication
import dev.carraes.lam.MainActivity
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.JsonPrimitive
import okhttp3.mockwebserver.*
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.ConcurrentLinkedQueue
import java.util.concurrent.ConcurrentHashMap

class ArticleLinksNavigationTest {
    @Test fun stableLinksWaitForPairingOpenColdAndWarmAndDoNotReplayAcrossRotation() = runBlocking {
        val application = ApplicationProvider.getApplicationContext<LamApplication>()
        val credentials = application.container.credentialStore
        credentials.clear()
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val first = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
        val second = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        val requests = ConcurrentLinkedQueue<RecordedRequest>()
        val canonical = ConcurrentHashMap<String, Article>()
        fun initial(id: String) = Article(id, "Linked report $id", "Summary", "Agent", "test", "lam",
            "2026-09-08T00:00:00Z", null, 0,
            listOf(ArticleAsset("index.html", "text/html", 1, "0".repeat(64), "inline")))
        fun article(id: String) = canonical.getOrPut(id) { initial(id) }
        fun intent(uri: String) = Intent(Intent.ACTION_VIEW, Uri.parse(uri), application, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        fun contains(node: AccessibilityNodeInfo?, title: String): Boolean = node != null &&
            (node.text?.toString() == title || (0 until node.childCount).any { contains(node.getChild(it), title) })
        fun shown(title: String) {
            val until = SystemClock.uptimeMillis() + 8_000
            while (SystemClock.uptimeMillis() < until) {
                if (contains(instrumentation.uiAutomation.rootInActiveWindow, title)) return
                Thread.sleep(50)
            }
            fail("Missing $title")
        }
        MockWebServer().use { server ->
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse {
                    requests.add(request)
                    val path = request.requestUrl!!.encodedPath
                    val id = if (path.contains(second)) second else first
                    val payload = when {
                        path == "/v2/events" -> return MockResponse().withWebSocketUpgrade(object : okhttp3.WebSocketListener() {})
                        path.endsWith("/content") -> articleJson.encodeToString(ArticleContent(article(id), listOf(JsonPrimitive("<p>Verified linked content</p>"))))
                        path.endsWith("/read") -> {
                            val body = org.json.JSONObject(request.body.readUtf8())
                            val old = article(id)
                            if (body.getLong("version") != old.version) return MockResponse().setResponseCode(409)
                            val updated = old.copy(version = old.version + 1, readAt = if (body.getBoolean("read")) "2026-09-08T00:00:01Z" else null)
                            canonical[id] = updated
                            articleJson.encodeToString(updated)
                        }
                        path == "/v2/articles" -> articleJson.encodeToString(ArticlePage(listOf(article(first), article(second)), null))
                        path.startsWith("/v2/articles/") -> articleJson.encodeToString(article(id))
                        else -> "[]"
                    }
                    return MockResponse().setBody(payload)
                }
            }
            try {
                ActivityScenario.launch<MainActivity>(intent("lam://articles/$first")).use { scenario ->
                    shown("Scan pairing code")
                    scenario.onActivity { assertEquals(first, ViewModelProvider(it)[ArticleLinks::class.java].pending.value) }
                    assertTrue(requests.isEmpty())
                    scenario.recreate()
                    scenario.onActivity { assertEquals(first, ViewModelProvider(it)[ArticleLinks::class.java].pending.value) }
                    credentials.save(PairedServer(server.url("/").toString(), "link-test-device", "Test"), "link-test-credential")
                    shown("Linked report $first")
                    shown("Read state saved.")
                    scenario.onActivity { assertNull(ViewModelProvider(it)[ArticleLinks::class.java].pending.value) }
                    application.startActivity(intent("lam://articles/$second"))
                    shown("Linked report $second"); shown("Read state saved.")
                    // Another reader marks this same canonical Article unread while its reader remains open.
                    canonical[second] = article(second).copy(version = 2, readAt = null)
                    application.container.articleRepository.refresh()
                    assertEquals(2L, application.container.articleRepository.state.value.items.single { it.id == second }.version)
                    val contentBefore = requests.count { it.path?.endsWith("/content") == true }
                    val writesBefore = requests.count { it.method == "PUT" }
                    application.startActivity(intent("lam://articles/$second"))
                    instrumentation.waitForIdleSync()
                    scenario.recreate(); shown("Linked report $second")
                    for (bad in listOf("lam://articles/$first?token=invalid", "lam://user@articles/$first", "lam://articles/$first/extra")) {
                        application.startActivity(intent(bad)); instrumentation.waitForIdleSync()
                        scenario.onActivity { assertNull(ViewModelProvider(it)[ArticleLinks::class.java].pending.value) }
                    }
                    shown("Linked report $second")
                    assertEquals(contentBefore, requests.count { it.path?.endsWith("/content") == true })
                    assertEquals(writesBefore, requests.count { it.method == "PUT" })
                    assertEquals(2L, article(second).version)
                    assertNull("Activity recreation must preserve the newer explicit unread", article(second).readAt)
                    assertNull(application.container.articleRepository.state.value.items.single { it.id == second }.readAt)
                }
                ActivityScenario.launch<MainActivity>(intent("lam://articles/$first")).use {
                    shown("Linked report $first")
                    assertEquals(2, requests.count { it.path == "/v2/articles/$first/content" })
                }
                assertTrue(requests.all { it.getHeader("Authorization") == "Bearer link-test-credential" })
                assertTrue(requests.none { it.requestUrl!!.queryParameter("token") != null })
            } finally { credentials.clear() }
        }
    }
}
