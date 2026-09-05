package dev.carraes.lam

import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toPixelMap
import androidx.compose.ui.test.captureToImage
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.onNodeWithText
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MainActivityTest {
    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    @Before fun keepTestActivityAwake() {
        composeRule.runOnUiThread { composeRule.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }

    @Test
    fun launchesPairingOnGraphiteSurface() {
        composeRule.waitUntil(5_000) {
            composeRule.onAllNodes(androidx.compose.ui.test.hasText("Scan pairing code")).fetchSemanticsNodes().isNotEmpty()
        }
        composeRule.onNodeWithText("Scan pairing code").assertExists()

        val surface = composeRule.onNodeWithTag("pairing-surface").captureToImage().toPixelMap()
        assertEquals(Color(0xFF121212), surface[1, 1])
    }
}
