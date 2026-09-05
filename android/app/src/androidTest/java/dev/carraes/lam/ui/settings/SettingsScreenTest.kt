package dev.carraes.lam.ui.settings

import androidx.activity.ComponentActivity
import android.graphics.Bitmap
import androidx.compose.runtime.*
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import dev.carraes.lam.items.*
import dev.carraes.lam.security.PairedServer
import dev.carraes.lam.ui.LamNav
import dev.carraes.lam.ui.history.*
import dev.carraes.lam.ui.requests.RequestsState
import dev.carraes.lam.ui.theme.LamTheme
import java.time.Instant
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SettingsScreenTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @Before fun keepAwake() { compose.runOnUiThread { compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) } }

    @Test fun settingsShowsSafeMetadataAndRequiresSeparateLocalEraseConfirmation() {
        var state by mutableStateOf(SettingsState(device = PairedServer("https://example.com/", "device-id", "S24 Ultra"),
            sync = SyncState.Stale(Instant.parse("2026-09-04T12:00:00Z"), Exception("SECRET"))))
        var confirmations = 0
        var notificationLinks = 0
        var copies = 0
        compose.setContent { LamTheme { SettingsScreen(state, "1.0", false, {},
            { notificationLinks++ }, { copies++ }, { state = state.copy(confirmation = UnpairConfirmation.REMOTE) },
            { confirmations++; state = state.copy(confirmation = if (confirmations == 1) UnpairConfirmation.LOCAL_ERASE else null) },
            { state = state.copy(confirmation = null) }) } }
        compose.onNodeWithText("S24 Ultra").assertExists()
        compose.onNodeWithText("device-id").assertExists()
        compose.onNodeWithText("https://example.com").assertExists()
        compose.onNodeWithText("SECRET", substring = true).assertDoesNotExist()
        compose.onNodeWithText("Notifications are disabled. Requests still sync when you open or refresh the app.").assertExists()
        screenshot("task9-settings.png")
        compose.onNodeWithText("Android notification settings").performScrollTo().performClick()
        compose.onNodeWithText("Copy diagnostics").performScrollTo().performClick()
        compose.onNodeWithText("Unpair this device").performScrollTo().performClick()
        compose.runOnIdle { assertEquals(0, confirmations); assertEquals(1, copies); assertEquals(1, notificationLinks) }
        compose.onNodeWithText("Revoke and unpair").performClick()
        compose.onNodeWithText("The server could not confirm revocation. Erasing locally cannot revoke the remote credential. Use the CLI to revoke this device.").assertExists()
        screenshot("task9-local-unpair.png")
        compose.runOnIdle { assertEquals(1, confirmations) }
        compose.onNodeWithText("Erase local data").performClick()
        compose.runOnIdle { assertEquals(2, confirmations) }
    }

    @Test fun offlineNavigationReadsInertHistoryAndReturnsToCachedRequests() {
        val closed = Item("closed", "Builder", "Completed release", "Cached **history body**", "host", "project",
            PriorityDto.NORMAL, listOf("Approve"), emptyList(), null, null, "", StatusDto.RESOLVED,
            "Approved", null, ResponseByDto.CLI, "2026-09-03T12:00:00Z", "2026-09-04T12:00:00Z", null, 2)
        val open = closed.copy(id = "open", title = "Cached request", status = StatusDto.OPEN)
        compose.setContent { LamTheme { LamNav(RequestsState(items = listOf(open), loading = false, stale = true), {}, {}, {}, {}, {},
            historyContent = { onRequests, onSettings -> HistoryScreen(HistoryState(items = listOf(closed), stale = true, incomplete = true), {}, {}, {}, {}, {}, {}, onRequests, onSettings) }) } }
        compose.onNodeWithContentDescription("Switch queue").performClick()
        compose.onNodeWithText("History").performClick()
        compose.onNodeWithText("Completed release").assertExists()
        compose.onNodeWithTag("history:closed").assertHasNoClickAction()
        compose.onNodeWithText("Resolved via CLI").assertExists()
        compose.onNodeWithText("Approved").assertExists()
        compose.onNodeWithText("Cached history body").assertExists()
        compose.onNodeWithText("Approve").assertDoesNotExist()
        compose.onNodeWithText("Quick response").assertDoesNotExist()
        compose.onNodeWithText("Dismiss").assertDoesNotExist()
        compose.onNodeWithText("Searching cached history only. Results may be incomplete.").assertExists()
        screenshot("task9-offline-history.png")
        compose.onNodeWithContentDescription("Switch queue").performClick()
        compose.onNodeWithText("Requests").performClick()
        compose.onNodeWithText("Cached request", useUnmergedTree = true).assertExists()
    }

    private fun screenshot(name: String) {
        compose.waitForIdle()
        val bitmap = requireNotNull(InstrumentationRegistry.getInstrumentation().uiAutomation.takeScreenshot())
        File(compose.activity.externalCacheDir, name).outputStream().use { bitmap.compress(Bitmap.CompressFormat.PNG, 100, it) }
    }
}
