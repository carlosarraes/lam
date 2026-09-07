package dev.carraes.lam.ui.requests

import androidx.activity.ComponentActivity
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.SemanticsActions
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.unit.LayoutDirection
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.detail.*
import dev.carraes.lam.ui.theme.LamTheme
import java.time.Instant
import kotlinx.coroutines.flow.*
import org.junit.Assert.*
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
@OptIn(ExperimentalTestApi::class)
class RequestGesturesTest {
    @Test fun fyiHasOnlyDismissAccessibilityAndSwipeAction() {
        var quick = 0; var dismiss = 0; var opens = 0
        compose.setContent { LamTheme {
            RequestCard(gestureItem.copy(kind = ItemKindDto.FYI, recommendation = null), gestureNow,
                actionsEnabled = true, onQuickResponse = { quick++ }, onDismiss = { dismiss++ }, onOpen = { opens++ })
        } }
        val card = compose.onNodeWithTag("request-card-request")
        card.assert(hasContentDescription("Normal priority", substring = true))
        val actions = card.fetchSemanticsNode().config[SemanticsActions.CustomActions]
        assertEquals(listOf("Dismiss"), actions.map { it.label })
        card.performTouchInput { swipe(Offset(width * .85f, centerY), Offset(width * .15f, centerY)) }
        compose.runOnIdle { assertEquals(0, quick); assertEquals(0, opens) }
        card.performTouchInput { swipe(Offset(width * .15f, centerY), Offset(width * .85f, centerY)) }
        compose.runOnIdle { assertEquals(1, dismiss) }
        card.performTouchInput { longClick() }
        compose.onNodeWithText("Quick response").assertDoesNotExist()
        compose.onNodeWithText("Dismiss").assertExists()
    }
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()
    @Before fun keepAwake() { compose.runOnUiThread { compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) } }

    @Test fun physicalRightAndLeftWorkInRtlAndOnlyAfterReleasePastThreshold() {
        var quick = 0; var dismiss = 0; var opens = 0
        compose.setContent { CompositionLocalProvider(LocalLayoutDirection provides LayoutDirection.Rtl) { LamTheme {
            RequestCard(gestureItem, gestureNow, actionsEnabled = true, onQuickResponse = { quick++ }, onDismiss = { dismiss++ }, onOpen = { opens++ })
        } } }
        val card = compose.onNodeWithTag("request-card-request")
        card.performTouchInput { down(center); moveBy(Offset(30f, 0f)); up() }
        compose.runOnIdle { assertEquals(0, quick); assertEquals(0, dismiss) }
        card.performTouchInput { down(center); moveBy(Offset(width * .4f, 0f)); cancel() }
        compose.runOnIdle { assertEquals(0, quick); assertEquals(0, dismiss) }
        card.performTouchInput { swipe(Offset(width * .15f, centerY), Offset(width * .85f, centerY)) }
        compose.runOnIdle { assertEquals(1, dismiss); assertEquals(0, quick) }
        card.performTouchInput { swipe(Offset(width * .85f, centerY), Offset(width * .15f, centerY)) }
        compose.runOnIdle { assertEquals(1, quick); assertEquals(1, dismiss); assertEquals(0, opens) }
        card.performClick()
        compose.runOnIdle { assertEquals(1, opens) }
    }

    @Test fun verticalScrollNeverActivatesResponseAndCardsReturnToRest() {
        var actions = 0
        compose.setContent { LamTheme { LazyColumn { items((0..25).toList()) { index ->
            RequestCard(gestureItem.copy(id = "$index", title = "Request $index"), gestureNow,
                actionsEnabled = true, onQuickResponse = { actions++ }, onDismiss = { actions++ }, onOpen = {})
        } } } }
        compose.onNodeWithTag("request-card-1").performTouchInput { swipeUp() }
        compose.runOnIdle { assertEquals(0, actions) }
        compose.onNodeWithTag("request-card-0").assertIsNotDisplayed()
    }

    @Test fun longPressAndTalkBackExposeBothActionsAndStaleStillOpens() {
        var quick = 0; var dismiss = 0; var opens = 0; var enabled by mutableStateOf(true)
        compose.setContent { LamTheme { RequestCard(gestureItem, gestureNow, actionsEnabled = enabled,
            onQuickResponse = { quick++ }, onDismiss = { dismiss++ }, onOpen = { opens++ }) } }
        val card = compose.onNodeWithTag("request-card-request")
        card.performTouchInput { longClick() }
        compose.onNodeWithText("Quick response").performClick()
        card.performTouchInput { longClick() }
        compose.onNodeWithText("Dismiss").performClick()
        val actions = card.fetchSemanticsNode().config[SemanticsActions.CustomActions]
        compose.runOnIdle {
            assertEquals(setOf("Quick response", "Dismiss"), actions.map { it.label }.toSet())
            actions.forEach { assertTrue(it.action()) }
        }
        compose.runOnIdle { assertEquals(2, quick); assertEquals(2, dismiss); assertEquals(0, opens); enabled = false }
        assertFalse(card.fetchSemanticsNode().config.contains(SemanticsActions.CustomActions))
        card.performTouchInput { swipeLeft() }
        compose.runOnIdle { assertEquals("A stale-card swipe must not open detail", 0, opens) }
        card.performTouchInput { swipeRight() }
        compose.runOnIdle { assertEquals("A stale-card swipe must not open detail", 0, opens) }
        card.performTouchInput { click() }
        compose.runOnIdle { assertEquals(2, quick); assertEquals(2, dismiss); assertEquals(1, opens) }
    }

    @Test fun quickChoiceAndFreeReplyRequireConfirmationAndRemoteClosureClearsSheet() {
        val repo = GestureRepository()
        lateinit var vm: DecisionViewModel
        compose.setContent {
            vm = remember { DecisionViewModel("request", repo) }
            LamTheme { DecisionDetailScreen(vm, onBack = {}) }
        }
        compose.waitUntil { vm.state.value.actionsEnabled }
        compose.runOnIdle { vm.quickResponse() }
        compose.onNodeWithTag("quick-choice-0").performClick()
        compose.onNodeWithText("Send \"Approve\"?").assertExists()
        compose.runOnIdle { assertTrue(repo.answers.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithTag("quick-reply").performClick()
        compose.onNodeWithTag("reply-input").performTextInput("Use the fallback")
        compose.onNodeWithText("Review reply").performScrollTo().performClick()
        compose.onNodeWithText("Send this reply?").assertExists()
        compose.runOnIdle { repo.current.value = gestureItem.copy(status = StatusDto.RESOLVED, responseChoice = "Reject", version = 2) }
        compose.onNodeWithText("Send this reply?").assertDoesNotExist()
        compose.onNodeWithTag("quick-choice-0").assertDoesNotExist()
        compose.onNodeWithTag("outcome").assertExists()
        compose.runOnIdle { assertTrue(repo.answers.isEmpty()) }
    }

    @Test fun detailOverflowDismissalCancelsAndThenConfirmsOnce() {
        val repo = GestureRepository()
        compose.setContent { val vm = remember { DecisionViewModel("request", repo) }; LamTheme { DecisionDetailScreen(vm, {}) } }
        compose.waitUntilAtLeastOneExists(hasText("Approve rollout"))
        compose.onNodeWithContentDescription("More actions").performClick()
        compose.onNodeWithText("Dismiss").performClick()
        compose.onNodeWithText("Dismiss this request?").assertExists()
        compose.runOnIdle { assertTrue(repo.answers.isEmpty()) }
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithContentDescription("More actions").performClick()
        compose.onNodeWithText("Dismiss").performClick()
        compose.onNodeWithText("Dismiss request").performClick()
        compose.runOnIdle { assertEquals(listOf(FinalAnswer.Dismiss), repo.answers) }
        compose.onNodeWithTag("outcome").assertExists()
    }
}

internal val gestureNow: Instant = Instant.parse("2026-09-04T12:00:00Z")
internal val gestureItem = Item("request", "Release agent", "Approve rollout", "Ready to deploy.", "host", "lam",
    PriorityDto.NORMAL, listOf("Approve", "Reject"), emptyList(), "Start gradually.", "Approve", "", StatusDto.OPEN,
    null, null, null, "2026-09-04T11:55:00Z", null, null, 1)

internal class GestureRepository : ItemRepository {
    val current = MutableStateFlow<Item?>(gestureItem)
    override val openItems = current.map { listOfNotNull(it) }
    override val syncState = MutableStateFlow<SyncState>(SyncState.Current(gestureNow))
    override val errors = emptyFlow<Exception>()
    val answers = mutableListOf<FinalAnswer>()
    var checkSucceeds = true
    override fun item(id: String) = current
    override suspend fun refreshItem(id: String) = true
    override suspend fun answer(id: String, answer: FinalAnswer): Boolean {
        answers += answer
        current.value = current.value!!.copy(status = if (answer == FinalAnswer.Dismiss) StatusDto.DISMISSED else StatusDto.RESOLVED,
            responseText = (answer as? FinalAnswer.Text)?.value, version = current.value!!.version + 1)
        return true
    }
    override suspend fun setCheck(id: String, index: Int, done: Boolean): Boolean {
        if (!checkSucceeds) return false
        val item = current.value!!
        val checks = item.checks.mapIndexed { i, check -> if (index == i) check.copy(done = done) else check }
        current.value = item.copy(checks = checks, version = item.version + 1, status = if (checks.all { it.done }) StatusDto.RESOLVED else StatusDto.OPEN)
        return true
    }
    override suspend fun refresh() = true
    override fun history(query: HistoryQuery) = flowOf(emptyList<Item>())
    override val cachedHistory = flowOf(emptyList<Item>())
    override suspend fun refreshHistory(query: HistoryQuery, cursor: String?) = HistoryResult(false, null)
    override suspend fun unpair() = Unit
    override suspend fun markSeen(id: String, version: Long) = false
}
