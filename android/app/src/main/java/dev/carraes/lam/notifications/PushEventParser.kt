package dev.carraes.lam.notifications

internal data class CriticalPushEvent(
    val event: Type,
    val itemId: String,
    val version: Int,
    val status: String,
    val kind: String,
    val name: String,
    val title: String,
) {
    enum class Type { CREATED, CHANGED, CLOSED }
}

internal object PushEventParser {
    private val statuses = setOf("open", "resolved", "dismissed", "retracted", "expired")
    private val kinds = setOf("request", "fyi")

    fun parse(data: Map<String, String>): CriticalPushEvent? {
        if (data["priority"] != "critical") return null
        val event = when (data["event"]) {
            "item.created" -> CriticalPushEvent.Type.CREATED
            "item.changed" -> CriticalPushEvent.Type.CHANGED
            "item.closed" -> CriticalPushEvent.Type.CLOSED
            else -> return null
        }
        val itemId = data["item_id"]?.takeIf(String::isNotBlank) ?: return null
        val version = data["version"]?.toIntOrNull()?.takeIf { it >= 0 } ?: return null
        val status = data["status"]?.takeIf(statuses::contains) ?: return null
        val kind = data["kind"]?.takeIf(kinds::contains) ?: return null
        val title = data["title"]?.takeIf(String::isNotBlank) ?: return null
        return CriticalPushEvent(event, itemId, version, status, kind, data["name"].orEmpty(), title)
    }
}
