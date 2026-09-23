package dev.carraes.lam.notifications

import dev.carraes.lam.items.Item
import dev.carraes.lam.items.PriorityDto
import dev.carraes.lam.items.StatusDto

internal sealed interface PushNotificationPlan {
    data class Show(
        val tag: String,
        val title: String,
        val text: String,
        val publicText: String,
        val itemId: String,
    ) : PushNotificationPlan

    data class Cancel(val tag: String) : PushNotificationPlan

    companion object {
        fun shouldRemain(item: Item?): Boolean =
            item?.priority == PriorityDto.CRITICAL && item.status == StatusDto.OPEN

        fun forEvent(event: CriticalPushEvent): PushNotificationPlan {
            val tag = "item:${event.itemId}"
            if (event.event == CriticalPushEvent.Type.CLOSED || event.status != "open") return Cancel(tag)
            val source = event.name.ifBlank { "agent" }
            val type = if (event.kind == "fyi") "Critical FYI" else "Critical request"
            return Show(tag, event.title, "$source · $type", "Critical lam from $source", event.itemId)
        }
    }
}
