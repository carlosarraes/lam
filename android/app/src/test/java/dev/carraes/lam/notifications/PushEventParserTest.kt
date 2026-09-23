package dev.carraes.lam.notifications

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class PushEventParserTest {
    private val valid = mapOf(
        "event" to "item.created",
        "item_id" to "item-1",
        "version" to "2",
        "status" to "open",
        "kind" to "request",
        "name" to "pm",
        "priority" to "critical",
        "title" to "Release decision",
    )

    @Test
    fun `parses a critical item invalidation`() {
        assertEquals(
            CriticalPushEvent(
                event = CriticalPushEvent.Type.CREATED,
                itemId = "item-1",
                version = 2,
                status = "open",
                kind = "request",
                name = "pm",
                title = "Release decision",
            ),
            PushEventParser.parse(valid),
        )
    }

    @Test
    fun `rejects messages that cannot represent a critical lam`() {
        listOf(
            valid - "item_id",
            valid + ("item_id" to ""),
            valid + ("version" to "-1"),
            valid + ("version" to "nope"),
            valid + ("event" to "article.published"),
            valid + ("status" to "unknown"),
            valid + ("kind" to "article"),
            valid + ("priority" to "normal"),
            valid + ("title" to ""),
        ).forEach { assertNull(PushEventParser.parse(it)) }
    }
}
