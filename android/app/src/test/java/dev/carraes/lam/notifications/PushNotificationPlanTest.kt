package dev.carraes.lam.notifications

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import dev.carraes.lam.items.Item
import dev.carraes.lam.items.ItemKindDto
import dev.carraes.lam.items.PriorityDto
import dev.carraes.lam.items.StatusDto

class PushNotificationPlanTest {
    private fun event(type: CriticalPushEvent.Type, status: String = "open", kind: String = "request") = CriticalPushEvent(
        event = type,
        itemId = "abc29",
        version = 1,
        status = status,
        kind = kind,
        name = "pm",
        title = "Release decision",
    )

    @Test
    fun `open critical requests show private stable notifications`() {
        assertEquals(
            PushNotificationPlan.Show(
                tag = "item:abc29",
                title = "Release decision",
                text = "pm · Critical request",
                publicText = "Critical lam from pm",
                itemId = "abc29",
            ),
            PushNotificationPlan.forEvent(event(CriticalPushEvent.Type.CREATED)),
        )
    }

    @Test
    fun `critical FYIs are labelled without pretending they need a decision`() {
        assertEquals(
            PushNotificationPlan.Show(
                tag = "item:abc29",
                title = "Release decision",
                text = "pm · Critical FYI",
                publicText = "Critical lam from pm",
                itemId = "abc29",
            ),
            PushNotificationPlan.forEvent(event(CriticalPushEvent.Type.CHANGED, kind = "fyi")),
        )
    }

    @Test
    fun `closed critical items cancel their stable notification`() {
        assertEquals(
            PushNotificationPlan.Cancel("item:abc29"),
            PushNotificationPlan.forEvent(event(CriticalPushEvent.Type.CLOSED, status = "resolved")),
        )
    }

    @Test
    fun `canonical state keeps only open critical notifications`() {
        val item = Item(
            id = "abc29", name = "pm", title = "Release decision", body = "", sourceHost = "box",
            sourceProject = "lam", priority = PriorityDto.CRITICAL, choices = emptyList(), checks = emptyList(),
            recommendation = null, recommendedChoice = null, link = "", status = StatusDto.OPEN,
            responseChoice = null, responseText = null, responseBy = null, createdAt = "2026-09-22T00:00:00Z",
            resolvedAt = null, expiresAt = null, version = 0, kind = ItemKindDto.REQUEST,
        )

        assertTrue(PushNotificationPlan.shouldRemain(item))
        assertFalse(PushNotificationPlan.shouldRemain(item.copy(status = StatusDto.RESOLVED)))
        assertFalse(PushNotificationPlan.shouldRemain(item.copy(priority = PriorityDto.NORMAL)))
        assertFalse(PushNotificationPlan.shouldRemain(null))
    }
}
