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
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MainActivityTest {
    @get:Rule
    val composeRule = createAndroidComposeRule<MainActivity>()

    @Test
    fun launchesRequestsOnGraphiteSurface() {
        composeRule.onNodeWithText("Requests").assertExists()

        val surface = composeRule.onNodeWithTag("requests-surface").captureToImage().toPixelMap()
        assertEquals(Color(0xFF121212), surface[1, 1])
    }
}
