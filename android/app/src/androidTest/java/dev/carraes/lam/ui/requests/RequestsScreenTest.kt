package dev.carraes.lam.ui.requests

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.*
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.LamNav
import dev.carraes.lam.ui.theme.LamTheme
import java.io.File
import java.time.Instant
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class RequestsScreenTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    private val now = Instant.parse("2026-09-04T12:00:00Z")

    @Before fun keepAwake() {
        compose.runOnUiThread {
            compose.activity.enableEdgeToEdge(SystemBarStyle.dark(0xFF121212.toInt()), SystemBarStyle.dark(0xFF121212.toInt()))
            compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    @Test fun loadingEmptyAndOfflineStatesAreDistinctAndRefreshRemainsAccessible() {
        var state by mutableStateOf(RequestsState(now = now))
        var refreshes = 0
        compose.setContent { LamTheme { LamNav(state, {}, {}, {}, {}, { refreshes++ }) } }
        compose.onNodeWithText("Loading requests…").assertExists()
        compose.runOnIdle { state = state.copy(loading = false, actionsEnabled = true) }
        compose.onNodeWithText("No open requests").assertExists()
        compose.runOnIdle { state = state.copy(stale = true, actionsEnabled = false, lastSuccess = now) }
        compose.onNodeWithText("Offline · replies unavailable").assertExists()
        compose.onNodeWithText("Could not load requests").assertExists()
        compose.onNodeWithText("Retry sync").performClick()
        compose.runOnIdle { assertEquals(1, refreshes) }
    }

    @Test fun cardReadsTitleAgentPriorityProgressAndOpenActionWithoutBodyPreview() {
        val decision = item("decision", "Approve the release", "Builder", PriorityDto.CRITICAL)
        val checklist = item("checks", "Release checklist", "Reviewer").copy(checks = listOf(CheckDto("Tests", true, null), CheckDto("Deploy", false, null)))
        compose.setContent { LamTheme { LamNav(RequestsState(items = listOf(decision, checklist), loading = false,
            stale = true, now = now), {}, {}, {}, {}, {}) } }
        compose.onNodeWithText("Private body text", useUnmergedTree = true).assertDoesNotExist()
        compose.onNodeWithContentDescription("Approve the release, Builder, Critical priority, Decision, 2 choices, No recommendation, Open request").assertHasClickAction()
        compose.onNodeWithContentDescription("Release checklist, Reviewer, Normal priority, Checklist, 1 of 2 complete, Open request").assertHasClickAction()
        compose.onAllNodesWithText("No recommendation", useUnmergedTree = true).assertCountEquals(1)
        compose.onNodeWithContentDescription("Approve the release, Builder, Critical priority, Decision, 2 choices, No recommendation, Open request").performClick()
        compose.onNodeWithText("Request detail").assertExists()
        compose.onNodeWithText("decision").assertExists()
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithText("Approve the release", useUnmergedTree = true).assertExists()
    }

    @Test fun titleSwitchesHistoryAndOverflowSettingsAndSearchAndFiltersRemainUsable() {
        var state by mutableStateOf(RequestsState(loading = false, now = now))
        compose.setContent { LamTheme { LamNav(state,
            { state = state.copy(query = it) }, { state = state.copy(type = it) },
            { state = state.copy(priority = it) }, { state = state.copy(type = null, priority = null) }, {}) } }
        compose.onNodeWithContentDescription("Search requests").performClick()
        compose.onNodeWithText("Search title, agent, or body").performTextInput("deployment")
        compose.runOnIdle { assertEquals("deployment", state.query) }
        compose.onNodeWithContentDescription("Clear search").performClick()
        compose.onNodeWithContentDescription("Filters").performClick()
        compose.onNodeWithText("Checklist").performClick()
        compose.onNodeWithText("Critical").performClick()
        compose.runOnIdle { assertEquals(ItemTypeDto.CHECKLIST, state.type); assertEquals(PriorityDto.CRITICAL, state.priority) }
        compose.onNodeWithText("Done").performClick()
        compose.onNodeWithContentDescription("Switch queue").performClick()
        compose.onNodeWithText("History").performClick()
        compose.onNodeWithText("History will be available in a later update.").assertExists()
        compose.onNodeWithContentDescription("More options").performClick()
        compose.onNodeWithText("Settings").performClick()
        compose.onNodeWithText("Settings will be available in a later update.").assertExists()
        compose.onNodeWithContentDescription("Back").performClick()
        compose.onNodeWithContentDescription("Switch queue").performClick()
        compose.onNodeWithText("Requests").performClick()
        compose.onNodeWithText("No matching requests").assertExists()
    }

    @Test fun seededQueueScreenshot() {
        val examples = listOf(
            item("deploy", "Approve production rollout", "Release agent", PriorityDto.CRITICAL).copy(recommendation = "Roll out gradually"),
            item("review", "Choose the retry policy", "Code reviewer").copy(recommendation = "Use bounded retries"),
            item("checks", "Verify the Android build", "Build agent").copy(checks = listOf(CheckDto("JVM tests", true, null), CheckDto("Lint", true, null), CheckDto("Device smoke", false, null))),
            item("legacy", "Keep the compatibility shim?", "Migration agent", PriorityDto.LOW),
        )
        compose.setContent { LamTheme { LamNav(RequestsState(items = examples, loading = false, actionsEnabled = true, now = now), {}, {}, {}, {}, {}) } }
        compose.onNodeWithText("Approve production rollout", useUnmergedTree = true).assertExists()
        val bitmap = compose.onRoot().captureToImage().asAndroidBitmap()
        File(compose.activity.externalCacheDir, "task6-requests.png").outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }

    @Test fun pullingTheQueueReconcilesEvenWhenItIsEmpty() {
        var refreshes = 0
        compose.setContent { LamTheme { LamNav(RequestsState(loading = false, now = now), {}, {}, {}, {}, { refreshes++ }) } }
        compose.onNodeWithTag("requests-surface").performTouchInput {
            swipeDown(startY = height * 0.3f, endY = height * 0.8f, durationMillis = 500)
        }
        compose.runOnIdle { assertEquals(1, refreshes) }
    }

    private fun item(id: String, title: String, name: String, priority: PriorityDto = PriorityDto.NORMAL) = Item(
        id, name, title, "Private body text", "host", "project", priority, listOf("Approve", "Reject"), emptyList(),
        null, null, "", StatusDto.OPEN, null, null, null, "2026-09-04T11:55:00Z", null, null, 1)
}
