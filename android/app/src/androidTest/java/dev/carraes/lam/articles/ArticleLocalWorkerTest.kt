package dev.carraes.lam.articles

import android.content.Intent
import android.net.Uri
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.platform.app.InstrumentationRegistry
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import dev.carraes.lam.LamApplication
import dev.carraes.lam.MainActivity
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.runBlocking
import okhttp3.HttpUrl.Companion.toHttpUrl
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Optional local Worker acceptance, invoked by article-viewer.browser.mjs with synthetic credentials. */
class ArticleLocalWorkerTest {
    @Test fun readsTheRealCliPublishedStaticFixture() = runBlocking {
        val args = InstrumentationRegistry.getArguments()
        val origin = args.getString("articleOrigin")
        val id = args.getString("articleId")
        assumeTrue("Requires the isolated local Worker fixture runner", origin != null && id != null)
        require(origin!!.startsWith("http://10.0.2.2:"))
        val application = ApplicationProvider.getApplicationContext<LamApplication>()
        val credentials = application.container.credentialStore
        val automation = InstrumentationRegistry.getInstrumentation().uiAutomation
        val repository = application.container.articleRepository
        val independentClient = OkHttpArticleApi(origin.toHttpUrl(), { "browser-test-token" })
        fun eventually(message: String, condition: () -> Boolean) {
            val until = SystemClock.uptimeMillis() + 15_000
            while (SystemClock.uptimeMillis() < until) {
                if (condition()) return
                Thread.sleep(50)
            }
            fail(message)
        }
        fun reconciled(expected: Article) = eventually("Native foreground list did not reconcile canonical version ${expected.version}") {
            repository.state.value.items.any { it.id == expected.id && it.version == expected.version && it.readAt == expected.readAt }
        }
        fun contains(node: AccessibilityNodeInfo?, text: String): Boolean = node != null &&
            (node.text?.toString()?.contains(text) == true || (0 until node.childCount).any { contains(node.getChild(it), text) })
        fun shown(text: String) {
            val until = SystemClock.uptimeMillis() + 15_000
            while (SystemClock.uptimeMillis() < until) {
                if (contains(automation.rootInActiveWindow, text)) return
                Thread.sleep(50)
            }
            fail("Missing local fixture text: $text")
        }
        credentials.clear()
        try {
            credentials.save(PairedServer(origin, "local-fixture", "Local Worker"), "browser-test-token")
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse("lam://articles/$id"), application, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            ActivityScenario.launch<MainActivity>(intent).use { scenario ->
                scenario.onActivity {
                    it.setTurnScreenOn(true)
                    it.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
                }
                shown("Show-me delivery report")
                shown("Read state saved.")
                shown("Prepare report")
                shown("Article bundle")
                shown("Reader behavior")
                shown("show-me.html")
                shown("Static report")
                shown("Inline image")
                shown("Save attachment")
                val screenshot = automation.takeScreenshot()
                java.io.File(application.getExternalFilesDir(null), "article-show-me-worker.png").outputStream().use {
                    assertTrue(screenshot.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it))
                }
                screenshot.recycle()
                scenario.recreate()
                shown("Show-me delivery report")
                shown("Read state saved.")

                // Independent client writes exercise the real Worker's /v2/events fanout and existing native collector.
                // No repository refresh is called: the open foreground session must invalidate and GET canonical state.
                val initial = independentClient.get(id!!)
                assertNotNull(initial.readAt)
                reconciled(initial)
                val unread = independentClient.setRead(id, false, initial.version)
                reconciled(unread)
                val read = independentClient.setRead(id, true, unread.version)
                reconciled(read)

                scenario.moveToState(Lifecycle.State.CREATED)
                eventually("Process must stop before the disconnected mutation") {
                    !ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
                }
                val missed = independentClient.setRead(id, false, read.version)
                assertEquals("The stopped native session has not refreshed", read.version,
                    repository.state.value.items.single { it.id == id }.version)
                scenario.moveToState(Lifecycle.State.RESUMED)
                reconciled(missed)
                val restored = independentClient.setRead(id, true, missed.version)
                reconciled(restored)
            }
        } finally { credentials.clear() }
    }
}
