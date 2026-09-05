package dev.carraes.lam.ui.requests

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.compose.runtime.*
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.detail.*
import dev.carraes.lam.ui.theme.LamTheme
import java.io.File
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@OptIn(ExperimentalTestApi::class)
class ChecklistInteractionsTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @Before fun keepAwake() { compose.runOnUiThread { compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) } }

    @Test fun checklistTickUntickKeepsScrollPositionAndMinimumTargets() {
        val repo = GestureRepository().apply { current.value = gestureItem.copy(choices = emptyList(), checks = (0..24).map { CheckDto("Step $it", false, null) }) }
        lateinit var vm: DecisionViewModel
        compose.setContent {
            vm = remember { DecisionViewModel("request", repo) }
            val checks = remember { ChecklistViewModel("request", repo) }
            LamTheme { DecisionDetailScreen(vm, {}, checks) }
        }
        compose.waitUntil { vm.state.value.actionsEnabled }
        val row = compose.onNodeWithTag("check-14")
        row.performScrollTo()
        val before = compose.onNodeWithTag("detail-scroll").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()
        row.assertHeightIsAtLeast(48.dp)
        row.performClick().assertIsOn()
        val after = compose.onNodeWithTag("detail-scroll").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value()
        assertEquals(before, after, .1f)
        row.performClick().assertIsOff()
        compose.runOnIdle { repo.current.value = repo.current.value!!.copy(checks = repo.current.value!!.checks.mapIndexed { i, c -> if (i == 2) c.copy(done = true) else c }) }
        assertEquals(before, compose.onNodeWithTag("detail-scroll").fetchSemanticsNode().config[SemanticsProperties.VerticalScrollAxisRange].value(), .1f)
        screenshot("task8-checklist.png")
    }

    @Test fun quickChecklistShowsRemainingChecksAndReplyInsteadThenFinalClosure() {
        val repo = GestureRepository().apply { current.value = gestureItem.copy(choices = emptyList(), checks = listOf(CheckDto("Already done", true, null), CheckDto("Verify release", false, null))) }
        lateinit var vm: DecisionViewModel
        compose.setContent {
            vm = remember { DecisionViewModel("request", repo) }
            val checks = remember { ChecklistViewModel("request", repo) }
            LamTheme { DecisionDetailScreen(vm, {}, checks) }
        }
        compose.waitUntil { vm.state.value.actionsEnabled }
        compose.runOnIdle { vm.quickResponse() }
        compose.onNodeWithTag("quick-check-0").assertDoesNotExist()
        compose.onNodeWithTag("quick-check-1").assertIsOff()
        compose.onNodeWithTag("quick-reply").performClick()
        compose.onNodeWithTag("reply-input").assertExists()
        compose.runOnIdle { vm.dismissReply() }
        screenshot("task8-quick-checklist.png", hasTestTag("quick-check-1"))
        compose.onNodeWithTag("quick-check-1").performClick()
        compose.onNodeWithTag("quick-check-1").assertDoesNotExist()
        compose.onNodeWithTag("outcome").assertExists()
        compose.runOnIdle { assertEquals(StatusDto.RESOLVED, repo.current.value?.status) }
    }

    @Test fun quickPlainFocusesReplyAndUsesSharedUtf8Validation() {
        val repo = GestureRepository().apply { current.value = gestureItem.copy(choices = emptyList()) }
        lateinit var vm: DecisionViewModel
        compose.setContent { vm = remember { DecisionViewModel("request", repo) }; LamTheme { DecisionDetailScreen(vm, {}) } }
        compose.waitUntil { vm.state.value.actionsEnabled }
        compose.runOnIdle { vm.quickResponse() }
        compose.onNodeWithTag("reply-input").assertIsFocused()
        compose.onNodeWithTag("reply-input").performTextInput("é".repeat(4097))
        compose.onNodeWithText("Review reply").performScrollTo().assertIsNotEnabled()
        compose.onNodeWithTag("reply-input").performTextReplacement("Proceed carefully")
        compose.onNodeWithText("Review reply").performScrollTo().performClick()
        screenshot("task8-quick-reply.png", hasText("Send this reply?"))
        compose.onNodeWithText("Send reply").performClick()
        compose.runOnIdle { assertEquals(listOf(FinalAnswer.Text("Proceed carefully")), repo.answers) }
    }

    @Test fun plainQuickCancellationClosesTheSheetAndChecklistFailureIsVisible() {
        val repo = GestureRepository().apply { current.value = gestureItem.copy(choices = emptyList()) }
        lateinit var vm: DecisionViewModel
        lateinit var checks: ChecklistViewModel
        compose.setContent {
            vm = remember { DecisionViewModel("request", repo) }
            checks = remember { ChecklistViewModel("request", repo) }
            LamTheme { DecisionDetailScreen(vm, {}, checks) }
        }
        compose.waitUntil { vm.state.value.actionsEnabled }
        compose.runOnIdle { vm.quickResponse() }
        compose.onNodeWithTag("reply-input").assertExists()
        compose.runOnIdle { vm.dismissReply() }
        compose.onNodeWithTag("reply-input").assertDoesNotExist()
        compose.onNodeWithText("Quick response").assertDoesNotExist()
        compose.runOnIdle {
            repo.current.value = repo.current.value!!.copy(checks = listOf(CheckDto("Verify rollback", false, null)))
            repo.checkSucceeds = false
        }
        compose.onNodeWithTag("check-0").performScrollTo().performClick().assertIsOff()
        compose.onNodeWithText("Could not save the check. The current server state is shown.").assertIsDisplayed()
        compose.runOnIdle { assertNull(checks.state.value.failure) }
    }

    private fun screenshot(name: String, sheetContent: SemanticsMatcher? = null) {
        if (sheetContent != null) compose.onNode(sheetContent).assertIsDisplayed()
        compose.waitForIdle()
        // Capture the composed display, including the separate bottom-sheet window.
        val bitmap = requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
        File(compose.activity.externalCacheDir, name).outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }
}
