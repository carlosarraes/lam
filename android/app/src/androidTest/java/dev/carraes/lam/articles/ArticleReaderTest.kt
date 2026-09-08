package dev.carraes.lam.articles

import android.webkit.WebView
import androidx.activity.ComponentActivity
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.*
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.test.platform.app.InstrumentationRegistry
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.MockResponse
import okio.Buffer
import dev.carraes.lam.ui.theme.LamTheme
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import org.junit.Before
import java.util.concurrent.atomic.AtomicInteger

class ArticleReaderTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @Before fun keepReaderInteractive() {
        compose.runOnUiThread {
            compose.activity.setTurnScreenOn(true)
            compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    @Test fun nativeImagesAreAuthenticatedMissingImagesHaveFeedbackAndAttachmentsRequireSaveControl() {
        MockWebServer().use { server ->
            val pixel = android.graphics.Bitmap.createBitmap(2, 2, android.graphics.Bitmap.Config.ARGB_8888).apply { eraseColor(android.graphics.Color.MAGENTA) }
            val image = java.io.ByteArrayOutputStream().also { pixel.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
            pixel.recycle()
            val note = "download-only notes\n".toByteArray()
            val filename = "article-fixture-${java.util.UUID.randomUUID()}.txt"
            val article = Article("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Native article reader", "Summary", "Agent", "host", "lam",
                "2026-09-08T12:00:00Z", null, 0, listOf(
                    ArticleAsset("index.html", "text/html", 1, "0".repeat(64), "inline"),
                    ArticleAsset("pixel.png", "image/png", image.size.toLong(), sha256(image), "inline"),
                    ArticleAsset("missing.png", "image/png", image.size.toLong(), sha256(image), "inline"),
                    ArticleAsset(filename, "text/plain", note.size.toLong(), sha256(note), "attachment")))
            val session = ArticleSession("test-account", "test-epoch")
            val policy = ArticleResourcePolicy(session.epoch, article)
            val document = LoadedArticle(article, """<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1">
                <title>Native article reader</title><style>body{font:18px sans-serif;padding:16px}img{width:120px;height:48px}</style></head><body>
                <h1>A private report</h1><p>Text can be selected and enlarged.</p><img src="${policy.assetUrl(1)}"><img src="${policy.assetUrl(2)}">
                <p><a href="${server.url("/confirmed-external")}">Open local canary</a></p>
                <details><summary>More details</summary><p>Additional notes</p></details>
                <svg width="200" height="70"><rect width="200" height="70" fill="teal"/><text x="10" y="40" fill="white">Static SVG</text></svg>
                <p><span data-lam-attachment="3">Attachment label</span></p></body></html>""", session)
            server.enqueue(MockResponse().setBody(Buffer().write(image)))
            server.enqueue(MockResponse().setResponseCode(404))
            val api = OkHttpArticleApi(server.url("/"), { "native-test-credential" })
            val repository = ArticleRepository(EmptyStorage(), { api }, { session })
            val visible = AtomicInteger()
            compose.setContent { LamTheme { ArticleReader(ArticleReaderState(id = article.id, document = document), repository,
                { visible.incrementAndGet() }, {}, {}) } }
            try {
                compose.waitUntil(15_000) { visible.get() >= 1 && server.requestCount == 2 }
            } catch (failure: AssertionError) {
                throw AssertionError("Reader visible=${visible.get()}, native resource requests=${server.requestCount}", failure)
            }
            compose.onNodeWithText("Some images are unavailable.").assertExists()
            assertEquals("One visibility notification for the committed document", 1, visible.get())
            repeat(2) {
                val request = server.takeRequest()
                assertEquals("Bearer native-test-credential", request.getHeader("Authorization"))
                assertTrue(request.path in listOf("/v2/articles/${article.id}/assets/1", "/v2/articles/${article.id}/assets/2"))
                assertNull(request.getHeader("Cookie"))
                assertNull(request.getHeader("Referer"))
            }
            compose.onNodeWithText("Larger text").performClick()
            val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
            assertTrue("Disposable screen must be interactive for native gestures", compose.activity.getSystemService(android.os.PowerManager::class.java).isInteractive)
            val screenshot = automation.takeScreenshot()
            java.io.File(compose.activity.externalCacheDir, "article-reader-controls.png").outputStream().use {
                screenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
            }
            screenshot.recycle()
            compose.onNodeWithText("Save $filename").performScrollTo().performClick()
            // Opening the document picker still must not fetch private attachment bytes.
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString()?.contains("documentsui") == true }
            assertEquals(2, server.requestCount)
            automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            // Global actions are asynchronous. Finish cancellation before another Activity test starts.
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString() == compose.activity.packageName }
            // Complete a second picker operation and read the file written by the real provider.
            server.enqueue(MockResponse().setBody(Buffer().write(note)))
            compose.onNodeWithText("Save $filename").performScrollTo().performClick()
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString()?.contains("documentsui") == true }
            assertEquals(2, server.requestCount)
            fun find(node: android.view.accessibility.AccessibilityNodeInfo?, text: String): android.view.accessibility.AccessibilityNodeInfo? {
                if (node == null) return null
                if (node.text?.toString()?.equals(text, ignoreCase = true) == true) return node
                for (index in 0 until node.childCount) find(node.getChild(index), text)?.let { return it }
                return null
            }
            compose.waitUntil(5000) { find(automation.rootInActiveWindow, "Save") != null }
            assertTrue(find(automation.rootInActiveWindow, "Save")!!.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK))
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString() == compose.activity.packageName }
            compose.waitUntil(5000) { compose.onAllNodesWithText("Attachment saved.", useUnmergedTree = true).fetchSemanticsNodes().isNotEmpty() }
            assertEquals(3, server.requestCount)
            val download = server.takeRequest()
            assertEquals("/v2/articles/${article.id}/assets/3", download.path)
            assertEquals("Bearer native-test-credential", download.getHeader("Authorization"))
            val saved = automation.executeShellCommand("cat /sdcard/Download/$filename").use {
                android.os.ParcelFileDescriptor.AutoCloseInputStream(it).readBytes()
            }
            assertArrayEquals("The document provider persisted the authenticated attachment bytes", note, saved)
            compose.waitUntil(5000) { find(automation.rootInActiveWindow, "Open local canary") != null }
            val bounds = android.graphics.Rect()
            find(automation.rootInActiveWindow, "Open local canary")!!.getBoundsInScreen(bounds)
            automation.executeShellCommand("input tap ${bounds.centerX()} ${bounds.centerY()}").use {
                android.os.ParcelFileDescriptor.AutoCloseInputStream(it).readBytes()
            }
            compose.onNodeWithText("Open in browser").assertExists()
            assertEquals("Destination confirmation does not navigate", 3, server.requestCount)
            server.enqueue(MockResponse().setBody("<html><body>Confirmed local canary</body></html>"))
            compose.onNodeWithText("Open in browser").performClick()
            compose.waitUntil(8000) { server.requestCount >= 4 }
            val external = server.takeRequest()
            assertEquals("/confirmed-external", external.path)
            assertNull(external.getHeader("Authorization"))
            assertNull(external.getHeader("Cookie"))
            assertNull(external.getHeader("Referer"))
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString() == "org.chromium.webview_shell" }
            automation.performGlobalAction(android.accessibilityservice.AccessibilityService.GLOBAL_ACTION_BACK)
            compose.waitUntil(5000) { automation.rootInActiveWindow?.packageName?.toString() == compose.activity.packageName }
        }
    }

    @Test fun staticReaderBlocksScriptsNetworkAndAttachmentLoadsButRendersSelectableZoomableContent() {
        MockWebServer().use { canary ->
            val asset = ArticleAsset("index.html", "text/html", 1, "0".repeat(64), "inline")
            val article = Article("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", "Static report", "Summary", "Agent", "host", "lam",
                "2026-09-08T12:00:00Z", null, 0, listOf(asset))
            val session = ArticleSession("test-account", "test-epoch")
            val html = """<!doctype html><html><head><meta name="viewport" content="width=device-width,initial-scale=1"><title>Static report</title>
                <style>body{font:20px sans-serif;padding:16px}p{background-image:url('${canary.url("/css")}')}</style></head><body>
                <h1>Private article</h1><p style="position:fixed;top:120px;left:24px;margin:0;z-index:1;background:white">Selectable report text</p>
                <div style="height:80px"></div><details open><summary>More details</summary><p>Expanded detail text</p></details>
                <svg width="220" height="100"><rect width="220" height="100" fill="teal"/><text x="10" y="50" fill="white">Static SVG</text></svg>
                <span data-lam-attachment="1">Download label is inert</span>
                <script>document.title='SCRIPT EXECUTED';fetch('${canary.url("/script")}')</script>
                <img src="${canary.url("/image")}"><iframe src="${canary.url("/frame")}"></iframe>
                <img src="file:///data/local/private"><img src="content://private/1"></body></html>"""
            val loaded = LoadedArticle(article, html, session)
            val repo = ArticleRepository(EmptyStorage(), { null }, { session })
            val visible = AtomicInteger()
            lateinit var web: WebView
            compose.setContent { AndroidView(modifier = Modifier.fillMaxSize(), factory = { context ->
                createArticleWebView(context, loaded, repo, { visible.incrementAndGet() }, {}, {}).also { web = it }
            }) }
            compose.waitUntil(15_000) { visible.get() == 1 }
            val drawn = AtomicInteger()
            compose.runOnIdle { web.postVisualStateCallback(1, object : WebView.VisualStateCallback() {
                override fun onComplete(requestId: Long) { drawn.incrementAndGet() }
            }) }
            compose.waitUntil(15000) { drawn.get() > 0 }
            compose.runOnIdle {
                assertEquals("Static report", web.title)
                assertFalse(web.settings.javaScriptEnabled)
                assertFalse(web.settings.allowFileAccess)
                assertFalse(web.settings.allowContentAccess)
                assertTrue(web.settings.blockNetworkLoads)
            }
            val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
            automation.serviceInfo = automation.serviceInfo.apply { flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS }
            val location = IntArray(2)
            var density = 1f
            compose.runOnIdle { web.getLocationOnScreen(location); density = web.resources.displayMetrics.density }
            val x = (location[0] + 50 * density).toInt()
            val y = (location[1] + 132 * density).toInt()
            automation.executeShellCommand("input touchscreen swipe $x $y $x $y 1200").use {
                android.os.ParcelFileDescriptor.AutoCloseInputStream(it).readBytes()
            }
            automation.takeScreenshot().also { shot ->
                java.io.File(compose.activity.externalCacheDir, "article-selection.png").outputStream().use {
                    shot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                }; shot.recycle()
            }
            compose.waitUntil(5000) { automation.windows.any { it.root?.findAccessibilityNodeInfosByText("Copy")?.isNotEmpty() == true } }
            val copy = automation.windows.flatMap { it.root?.findAccessibilityNodeInfosByText("Copy").orEmpty() }.first()
            assertTrue(copy.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK))
            compose.runOnIdle {
                val clipboard = compose.activity.getSystemService(android.content.ClipboardManager::class.java)
                assertTrue("Selection copied article text", clipboard.primaryClip?.getItemAt(0)?.text?.contains("Selectable") == true)
                assertTrue(web.zoomIn()); web.settings.textZoom = 125
            }
            val matches = AtomicInteger(-1)
            compose.runOnIdle { web.setFindListener { _, count, done -> if (done) matches.set(count) }; web.findAllAsync("Selectable report text") }
            compose.waitUntil(5000) { matches.get() >= 0 }
            assertEquals(1, matches.get())
            compose.runOnIdle { web.clearMatches() }
            assertEquals(0, canary.requestCount)
            val screenshot = InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot()
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            java.io.File(context.getExternalFilesDir(null), "article-reader.png").outputStream().use {
                assertTrue(screenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it))
            }
            screenshot.recycle()
            compose.runOnIdle { web.destroy() }
        }
    }

    private class EmptyStorage : ArticleStorage {
        override suspend fun list(account: String) = emptyList<ArticleEntity>()
        override suspend fun get(account: String, id: String): ArticleEntity? = null
        override suspend fun save(rows: List<ArticleEntity>, current: () -> Boolean) = false
    }
}
