package dev.carraes.lam.ui.detail

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.*
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.style.ResolvedTextDirection
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.theme.LamTheme
import java.io.File
import java.time.Instant
import kotlinx.coroutines.flow.*
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@OptIn(ExperimentalTestApi::class)
class DecisionDetailScreenTest {
    @Test fun cachedFyiLoadFailureKeepsBodyAndShowsRetryWithoutResponseControls() {
        val repository = DeviceDetailRepository().apply {
            current.value = example.copy(kind = ItemKindDto.FYI, choices = emptyList(), recommendation = null)
            refreshSucceeds = false
        }
        compose.setContent {
            val vm = remember { DecisionViewModel("request", repository) }
            LamTheme { DecisionDetailScreen(vm, {}) }
        }
        compose.onNodeWithText("Could not refresh this FYI. Cached content is shown.").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("markdown-body").assertExists()
        compose.onNodeWithText("Retry sync").assertExists()
        compose.onNodeWithTag("recommendation").assertDoesNotExist()
        compose.onNodeWithText("Done").assertDoesNotExist()
        compose.onNodeWithText("Write another reply").assertDoesNotExist()
        compose.runOnIdle { assertEquals(0, repository.seen) }
    }

    @Test fun failedFullDetailFyiDismissShowsErrorAndKeepsReadableBodyWithoutResponseControls() {
        val repository = DeviceDetailRepository().apply {
            current.value = example.copy(kind = ItemKindDto.FYI, choices = emptyList(), recommendation = null)
            seenSucceeds = false
            answerSucceeds = false
        }
        compose.setContent {
            val vm = remember { DecisionViewModel("request", repository) }
            LamTheme { DecisionDetailScreen(vm, {}) }
        }
        compose.waitUntilAtLeastOneExists(hasText("Could not mark this FYI as seen. Refresh to try again."))
        compose.onNodeWithContentDescription("More actions").performClick()
        compose.onNodeWithText("Quick response").assertDoesNotExist()
        compose.onNodeWithText("Dismiss").performClick()
        compose.onNodeWithText("Dismiss request").performClick()
        compose.onNodeWithText("Could not confirm dismissal. Refresh to check this FYI before trying again.").performScrollTo().assertIsDisplayed()
        compose.onNodeWithTag("markdown-body").assertExists()
        compose.onNodeWithTag("recommendation").assertDoesNotExist()
        compose.onNodeWithText("Done").assertDoesNotExist()
        compose.onNodeWithText("Write another reply").assertDoesNotExist()
        compose.runOnIdle { assertEquals(listOf(FinalAnswer.Dismiss), repository.answers) }
    }
    @Test fun openingFyiMarksSeenKeepsBodyAndOffersNoReplyControls() {
        val repository = DeviceDetailRepository().apply {
            current.value = example.copy(title = "Production rollout update", kind = ItemKindDto.FYI, choices = emptyList(), recommendation = null, recommendedChoice = null)
        }
        compose.setContent {
            val vm = remember { DecisionViewModel("request", repository) }
            LamTheme { DecisionDetailScreen(vm, onBack = {}) }
        }
        compose.waitUntilAtLeastOneExists(hasText("Seen"))
        compose.onNodeWithText("Normal").assertExists()
        compose.onNodeWithTag("markdown-body").assertExists()
        compose.onNodeWithTag("recommendation").assertDoesNotExist()
        compose.onNodeWithText("Done").assertDoesNotExist()
        compose.onNodeWithText("Write another reply").assertDoesNotExist()
        compose.runOnIdle { assertEquals(1, repository.seen); assertTrue(repository.answers.isEmpty()) }
        screenshot("task3-fyi-detail.png")
    }
    @Test fun plainDoneRequiresConfirmationAndCompletesWithoutInventingAReply() {
        val repository = DeviceDetailRepository()
        repository.current.value = example.copy(choices = emptyList(), checks = emptyList())
        compose.setContent {
            val vm = remember { DecisionViewModel("request", repository) }
            LamTheme { DecisionDetailScreen(vm, onBack = {}) }
        }
        compose.waitUntilAtLeastOneExists(hasText("Context"))
        compose.onNodeWithText("Done").performScrollTo().performClick()
        compose.onNodeWithText("Mark as done?").assertExists()
        compose.runOnIdle { assertTrue(repository.answers.isEmpty()) }
        compose.onNodeWithText("Confirm done").performClick()
        compose.onNodeWithTag("outcome").performScrollTo().assertExists()
        compose.runOnIdle { assertEquals(1, repository.answers.size) }
    }
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    @Before fun keepAwake() {
        compose.runOnUiThread {
            compose.activity.enableEdgeToEdge(SystemBarStyle.dark(0xFF121212.toInt()), SystemBarStyle.dark(0xFF121212.toInt()))
            compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    @Test fun recommendationBeforeBodyAndActionsWithAnExplicitRecommendedMarker() {
        compose.setContent { LamTheme { DecisionDetailScreen(ready(), onBack = {}) } }
        compose.waitUntilAtLeastOneExists(hasText("Context"))
        val recommendation = compose.onNodeWithTag("recommendation").fetchSemanticsNode().boundsInRoot
        val body = compose.onNodeWithText("Context").fetchSemanticsNode().boundsInRoot
        assertTrue(recommendation.bottom <= body.top)
        compose.onNodeWithTag("choice-0").performScrollTo().assertTextContains("Recommended")
        val action = compose.onNodeWithTag("choice-0").fetchSemanticsNode().boundsInRoot
        assertTrue(compose.onNodeWithText("Ready to deploy.").fetchSemanticsNode().boundsInRoot.bottom <= action.top)
        compose.onNodeWithTag("choice-1").assertTextContains("Reject")
        compose.onNodeWithText("No recommendation").assertDoesNotExist()
    }

    @Test fun legacyDecisionWarnsButChecklistDoesNot() {
        var item by mutableStateOf(example.copy(recommendation = null, recommendedChoice = null))
        compose.setContent { LamTheme { DecisionDetailScreen(ready(item), onBack = {}) } }
        compose.onNodeWithText("No recommendation").assertExists()
        compose.runOnIdle { item = item.copy(checks = listOf(CheckDto("Review", false, null))) }
        compose.onNodeWithText("No recommendation").assertDoesNotExist()
    }

    @Test fun choicesAndMultilineReplyRequireConfirmationAndResolveThroughViewModel() {
        val repository = DeviceDetailRepository()
        lateinit var vm: DecisionViewModel
        compose.setContent {
            vm = remember { DecisionViewModel("request", repository) }
            LamTheme { DecisionDetailScreen(vm, onBack = {}) }
        }
        compose.waitUntilAtLeastOneExists(hasText("Context"))
        compose.onNodeWithTag("choice-0").performScrollTo().performClick()
        compose.onNodeWithText("Send \"Approve\"?").assertExists()
        compose.runOnIdle { assertTrue(repository.answers.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithTag("choice-1").performScrollTo().performClick()
        compose.onNodeWithText("Send \"Reject\"?").assertExists()
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithText("Write another reply").performScrollTo().performClick()
        compose.onNodeWithTag("reply-input").performTextInput("Proceed\nKeep the fallback.")
        compose.onNodeWithText("Review reply").performScrollTo().performClick()
        compose.onNodeWithText("Send this reply?").assertExists()
        compose.runOnIdle { assertTrue(repository.answers.isEmpty()) }
        compose.onNodeWithText("Send reply").performClick()
        compose.onNodeWithTag("outcome").performScrollTo().assertExists()
        compose.onNodeWithText("Proceed\nKeep the fallback.").assertExists()
        compose.runOnIdle { assertEquals(listOf(FinalAnswer.Text("Proceed\nKeep the fallback.")), repository.answers) }
        compose.onNodeWithTag("choice-0").assertDoesNotExist()
    }

    @Test fun staleControlsDisableAndRemoteClosureDismissesConfirmationWithCanonicalOutcome() {
        val repository = DeviceDetailRepository()
        compose.setContent { val vm = remember { DecisionViewModel("request", repository) }; LamTheme { DecisionDetailScreen(vm, {}) } }
        compose.waitUntilAtLeastOneExists(hasText("Context"))
        compose.runOnIdle { repository.syncState.value = SyncState.Stale(now, Exception("offline")) }
        compose.onNodeWithTag("choice-0").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithText("Write another reply").assertIsNotEnabled()
        compose.runOnIdle { repository.syncState.value = SyncState.Current(now) }
        compose.onNodeWithTag("choice-0").performClick()
        compose.onNodeWithText("Send \"Approve\"?").assertExists()
        compose.runOnIdle { repository.current.value = example.copy(status = StatusDto.RESOLVED, responseChoice = "Reject", responseBy = ResponseByDto.CLI, version = 2) }
        compose.onNodeWithText("Send \"Approve\"?").assertDoesNotExist()
        compose.onNodeWithTag("outcome").performScrollTo().assertTextContains("Resolved via CLI")
        compose.onNodeWithText("Reject").assertExists()
        compose.onNodeWithTag("choice-0").assertDoesNotExist()
    }

    @Test fun markdownLinksShowActualDestinationBeforeOpeningAndUnsafeLinksStayBlocked() {
        val opened = mutableListOf<String>()
        compose.setContent { LamTheme { DecisionDetailScreen(ready(example.copy(body = "[Documentation](https://example.com/docs)", link = "javascript:alert(1)")), onBack = {}, onOpenLink = { opened += it; true }) } }
        compose.waitUntilAtLeastOneExists(hasText("Documentation"))
        compose.onNodeWithText("Documentation").performTouchInput { click(center) }
        compose.onNodeWithText("https://example.com/docs").assertExists()
        compose.runOnIdle { assertTrue(opened.isEmpty()) }
        compose.onNodeWithText("Open in browser").performClick()
        compose.runOnIdle { assertEquals(listOf("https://example.com/docs"), opened) }
        compose.onNodeWithText("Open related link").performScrollTo().performClick()
        compose.onNodeWithText("javascript:alert(1)").assertExists()
        compose.onNodeWithText("Open in browser").assertIsNotEnabled()
    }

    @Test fun htmlIsLiteralAndRemoteImagesNeverFetch() {
        val server = okhttp3.mockwebserver.MockWebServer()
        server.start()
        try {
            val markdown = "<script>alert('x')</script>\n\nInline <b>literal</b>.\n\n![remote image](${server.url("/secret.png")})"
            compose.setContent { LamTheme { DecisionDetailScreen(ready(example.copy(body = markdown)), onBack = {}) } }
            compose.waitUntilAtLeastOneExists(hasText("<script>alert('x')</script>", substring = true))
            compose.onNodeWithText("Inline <b>literal</b>.").assertExists()
            compose.onNodeWithTag("choice-0").performScrollTo()
            assertEquals(0, server.requestCount)
        } finally { server.shutdown() }
    }

    @Test fun longMarkdownTablesCodeAndLargeRtlTextStayReadableAndScrollable() {
        val markdown = "# Deployment review\n\n**Important** and *careful* with `inline code`.\n\n- First step\n- Second step\n\n" +
            "| Component | Result |\n| --- | --- |\n| A long component name that must wrap without ellipsis | Passed |\n\n" +
            "```sh\nlam push --title 'A long command that scrolls horizontally without shrinking its type size' --priority critical\n```\n\n" +
            (1..15).joinToString("\n\n") { "Paragraph $it. Read the rollout details before answering." } + "\n\n## النهاية"
        compose.setContent {
            val density = LocalDensity.current
            CompositionLocalProvider(LocalDensity provides Density(density.density, 1.6f), LocalLayoutDirection provides LayoutDirection.Rtl) {
                LamTheme { DecisionDetailScreen(ready(example.copy(body = markdown)), onBack = {}) }
            }
        }
        compose.waitUntilAtLeastOneExists(hasText("Deployment review"))
        val cell = compose.onNodeWithText("A long component name that must wrap without ellipsis", substring = true).performScrollTo()
        cell.performSemanticsAction(SemanticsActions.GetTextLayoutResult) { getLayout ->
            val layouts = mutableListOf<TextLayoutResult>()
            getLayout(layouts)
            assertTrue(layouts.single().lineCount > 1)
            assertFalse(layouts.single().hasVisualOverflow)
        }
        compose.onAllNodes(SemanticsMatcher.keyIsDefined(SemanticsProperties.HorizontalScrollAxisRange)).fetchSemanticsNodes().let { nodes ->
            assertTrue(nodes.any { it.config[SemanticsProperties.HorizontalScrollAxisRange].maxValue() > 0 })
        }
        compose.onNodeWithText("النهاية").performScrollTo().assertIsDisplayed()
        compose.onNodeWithText("Paragraph 15. Read the rollout details before answering.").performSemanticsAction(SemanticsActions.GetTextLayoutResult) { getLayout ->
            val layouts = mutableListOf<TextLayoutResult>()
            getLayout(layouts)
            assertEquals(ResolvedTextDirection.Ltr, layouts.single().getParagraphDirection(0))
        }
        compose.onNodeWithText("Write another reply").performScrollTo().assertIsDisplayed()
        screenshot("task7-detail-rtl.png")
    }

    @Test fun loadingMissingAndReadFailureHaveDistinctRetryStates() {
        var state by mutableStateOf(DecisionState())
        var retries = 0
        compose.setContent { LamTheme { DecisionDetailScreen(state, onBack = {}, onRefresh = { retries++ }) } }
        compose.onNodeWithText("Loading request…").assertExists()
        compose.runOnIdle { state = state.copy(loading = false, loadFailed = true) }
        compose.onNodeWithText("Could not load request").assertExists()
        compose.onNodeWithText("Request unavailable").assertDoesNotExist()
        compose.onNodeWithText("Retry sync").performClick()
        compose.runOnIdle { assertEquals(1, retries); state = state.copy(loadFailed = false) }
        compose.onNodeWithText("Request unavailable").assertExists()
    }

    @Test fun representativeDecisionScreenshot() {
        compose.setContent { LamTheme { DecisionDetailScreen(ready(example.copy(body = "## Rollout plan\n\nThe release passed **unit tests** and the device smoke run.\n\n- Start with the internal channel\n- Watch errors for 30 minutes\n- Expand when checks stay green\n\n`lam wait rollout` will receive your final answer.")), onBack = {}) } }
        compose.waitUntilAtLeastOneExists(hasText("Rollout plan"))
        screenshot("task7-detail.png")
    }

    private fun screenshot(name: String) {
        val bitmap = compose.onRoot().captureToImage().asAndroidBitmap()
        File(compose.activity.externalCacheDir, name).outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    private fun ready(item: Item = example) = DecisionState(item = item, loading = false, sync = SyncState.Current(now))
}

private val now: Instant = Instant.parse("2026-09-04T12:00:00Z")
private val example = Item("request", "Release agent", "Approve production rollout", "## Context\n\nReady to deploy.", "workstation", "lam",
    PriorityDto.NORMAL, listOf("Approve", "Reject"), emptyList(), "Start with the internal channel. The fallback is ready if errors rise.", "Approve", "", StatusDto.OPEN,
    null, null, null, "2026-09-04T11:55:00Z", null, null, 1)

private class DeviceDetailRepository : ItemRepository {
    var refreshSucceeds = true
    var answerSucceeds = true
    var seenSucceeds = true
    val current = MutableStateFlow<Item?>(example)
    override val openItems = current.map { listOfNotNull(it) }
    override val syncState = MutableStateFlow<SyncState>(SyncState.Current(now))
    override val errors = emptyFlow<Exception>()
    val answers = mutableListOf<FinalAnswer>()
    override fun item(id: String) = current
    override suspend fun refreshItem(id: String): Boolean {
        if (!refreshSucceeds) syncState.value = SyncState.Stale(now, ApiError.Transport("offline"))
        return refreshSucceeds
    }
    override suspend fun answer(id: String, answer: FinalAnswer): Boolean {
        answers += answer
        if (!answerSucceeds) return false
        current.value = current.value!!.copy(status = StatusDto.RESOLVED, responseText = (answer as? FinalAnswer.Text)?.value,
            responseChoice = (answer as? FinalAnswer.Choice)?.value, responseBy = ResponseByDto.PHONE, version = 2)
        return true
    }
    override suspend fun refresh() = true
    override fun history(query: HistoryQuery) = flowOf(emptyList<Item>())
    override val cachedHistory = flowOf(emptyList<Item>())
    override suspend fun refreshHistory(query: HistoryQuery, cursor: String?) = HistoryResult(false, null)
    override suspend fun setCheck(id: String, index: Int, done: Boolean) = false
    override suspend fun unpair() = Unit
    var seen = 0
    override suspend fun markSeen(id: String, version: Long): Boolean {
        seen++
        if (!seenSucceeds) return false
        current.value = current.value!!.copy(status = StatusDto.DISMISSED, seenAt = now.toString(), resolvedAt = now.toString(), version = version + 1)
        return true
    }
}
