package dev.carraes.lam.notifications

import android.app.Notification
import android.app.NotificationManager
import android.content.Context
import android.os.SystemClock
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Before
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class LamNotificationsTest {
    private val context = ApplicationProvider.getApplicationContext<Context>()
    private val manager = context.getSystemService(NotificationManager::class.java)
    private val notifications = LamNotifications(context)

    @Before
    fun allowNotifications() {
        InstrumentationRegistry.getInstrumentation().uiAutomation
            .executeShellCommand("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS").close()
        manager.cancelAll()
        waitUntil { manager.activeNotifications.none { it.tag?.startsWith("item:") == true } }
    }

    @After
    fun clear() {
        manager.cancelAll()
        waitUntil { manager.activeNotifications.none { it.tag?.startsWith("item:") == true } }
    }

    private fun waitUntil(predicate: () -> Boolean) {
        val deadline = SystemClock.uptimeMillis() + 2_000
        while (!predicate() && SystemClock.uptimeMillis() < deadline) SystemClock.sleep(25)
        check(predicate()) { "Notification manager did not reach the expected state" }
    }

    @Test
    fun createsAHighImportanceCriticalChannel() {
        notifications.createChannels()
        val channel = manager.getNotificationChannel(LamNotifications.CRITICAL_CHANNEL)
        assertEquals(NotificationManager.IMPORTANCE_HIGH, channel.importance)
    }

    @Test
    fun postsPrivateStableNotificationAndCancelsItWhenClosed() {
        val open = CriticalPushEvent(
            CriticalPushEvent.Type.CREATED, "abc29", 0, "open", "request", "pm", "Release decision",
        )
        notifications.handle(open)

        waitUntil { manager.activeNotifications.any { it.tag == "item:abc29" } }

        val posted = manager.activeNotifications.single { it.tag == "item:abc29" }.notification
        assertEquals(Notification.VISIBILITY_PRIVATE, posted.visibility)
        assertEquals("Release decision", posted.extras.getString(Notification.EXTRA_TITLE))
        assertEquals("pm · Critical request", posted.extras.getString(Notification.EXTRA_TEXT))
        assertEquals("Critical lam from pm", posted.publicVersion.extras.getString(Notification.EXTRA_TEXT))
        assertNotNull(posted.contentIntent)

        notifications.handle(open.copy(event = CriticalPushEvent.Type.CLOSED, status = "resolved", version = 1))
        waitUntil { manager.activeNotifications.none { it.tag == "item:abc29" } }
        assertEquals(0, manager.activeNotifications.count { it.tag == "item:abc29" })
    }

    @Test
    fun reconciliationCancelsOnlyNotificationsThatAreNoLongerOpen() {
        val first = CriticalPushEvent(
            CriticalPushEvent.Type.CREATED, "abc29", 0, "open", "request", "pm", "First decision",
        )
        val second = first.copy(itemId = "def34", title = "Second decision")
        notifications.handle(first)
        notifications.handle(second)

        waitUntil { manager.activeNotifications.count { it.tag?.startsWith("item:") == true } == 2 }

        notifications.reconcile(setOf(second.itemId))

        waitUntil { manager.activeNotifications.none { it.tag == "item:abc29" } }

        assertEquals(listOf("item:def34"), manager.activeNotifications.mapNotNull { it.tag }.sorted())
    }
}
