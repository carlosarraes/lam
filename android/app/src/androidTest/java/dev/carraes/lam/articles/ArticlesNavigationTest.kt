package dev.carraes.lam.articles

import android.graphics.Bitmap
import androidx.activity.ComponentActivity
import androidx.compose.runtime.*
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import dev.carraes.lam.ui.LamNav
import dev.carraes.lam.ui.history.HistoryScreen
import dev.carraes.lam.ui.history.HistoryState
import dev.carraes.lam.ui.requests.RequestsState
import dev.carraes.lam.ui.theme.LamTheme
import org.junit.Assert.*
import org.junit.Rule
import org.junit.Test
import java.io.File

class ArticlesNavigationTest {
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    @Test fun dayNavigationAndAllUnreadReturnToToday() {
        var state by mutableStateOf(ArticlesState())
        compose.setContent { LamTheme {
            ArticlesScreen(state, {}, { state = state.copy(readFilter = it) }, {}, {}, {}, {}, {}, {}, {},
                onDay = { state = state.copy(day = it) },
                onAllUnread = { state = state.copy(day = null, readFilter = "unread") })
        } }
        compose.onNodeWithContentDescription("Next day").assertIsNotEnabled()
        compose.onNodeWithContentDescription("Previous day").performClick()
        compose.onNodeWithText("Yesterday").assertExists()
        compose.onNodeWithContentDescription("Next day").performClick()
        compose.onNodeWithText("Today").assertExists()
        compose.onNodeWithText("All unread").performClick()
        compose.runOnIdle { assertNull(state.day); assertEquals("unread", state.readFilter) }
        compose.onNodeWithText("Today").performClick()
        compose.runOnIdle { assertEquals(articleToday().toString(), state.day) }
        compose.onNodeWithText("Today").performClick()
        compose.onNodeWithText("Cancel").assertExists().performClick()
    }

    @Test fun titleSwitcherPreservesRequestsHistoryAndArticleSearchSelections() {
        var requests by mutableStateOf(RequestsState(query = "request query", loading = false))
        var history by mutableStateOf(HistoryState())
        var articles by mutableStateOf(ArticlesState(items = listOf(Article("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
            "A private release report", "Build notes, screenshots, and the result of the release checks.", "Release agent", "workstation", "lam",
            "2026-09-08T12:00:00Z", null, 0, emptyList()))))
        compose.setContent { LamTheme { LamNav(requests, { requests = requests.copy(query = it) }, {}, {}, {}, {},
            historyContent = { onRequests, onSettings ->
                HistoryScreen(history, { history = history.copy(query = history.query.copy(query = it)) }, {}, {}, {}, {}, {}, onRequests, onSettings)
            }, articlesContent = { onRequests, onHistory, onSettings, onArticle ->
                ArticlesScreen(articles, { articles = articles.copy(query = it) }, { articles = articles.copy(readFilter = it) }, {}, {}, onArticle,
                    {}, onRequests, onHistory, onSettings)
            }) } }
        switch("History")
        compose.onNodeWithContentDescription("Search history").performClick()
        compose.onNodeWithText("Search title, agent, or body").performTextInput("history query")
        switch("Articles")
        compose.onNodeWithText("A private release report").assertExists()
        File(compose.activity.externalCacheDir, "articles-list.png").outputStream().use {
            compose.onRoot().captureToImage().asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it)
        }
        compose.onNodeWithContentDescription("Search articles").performClick()
        compose.onNode(hasSetTextAction()).performTextInput("article query")
        compose.onAllNodesWithText("Unread")[0].performClick()
        switch("Requests")
        compose.onNodeWithText("request query").assertExists()
        switch("History")
        compose.onNodeWithText("history query").assertExists()
        switch("Articles")
        compose.onNodeWithText("article query").assertExists()
        compose.runOnIdle { assertEquals("unread", articles.readFilter) }
    }

    private fun switch(destination: String) {
        compose.onNodeWithContentDescription("Switch queue").performClick()
        compose.onNodeWithText(destination).performClick()
    }
}
