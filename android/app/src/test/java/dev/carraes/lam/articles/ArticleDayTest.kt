package dev.carraes.lam.articles

import java.time.Instant
import java.time.LocalDate
import java.util.Locale
import org.junit.Assert.*
import org.junit.Test

class ArticleDayTest {
    @Test fun `display timestamp stays on selected day even when device uses UTC`() {
        val original = java.util.TimeZone.getDefault()
        try {
            java.util.TimeZone.setDefault(java.util.TimeZone.getTimeZone("UTC"))
            assertEquals("9/8/26, 11:59 PM", articleTime("2026-09-09T02:59:59Z", Locale.US).replace('\u202f', ' '))
            assertEquals("9/9/26, 12:00 AM", articleTime("2026-09-09T03:00:00Z", Locale.US).replace('\u202f', ' '))
        } finally { java.util.TimeZone.setDefault(original) }
    }
    @Test fun `São Paulo midnight defines today and relative labels`() {
        assertEquals(LocalDate.parse("2026-09-08"), articleToday(Instant.parse("2026-09-09T02:59:59Z")))
        val today = articleToday(Instant.parse("2026-09-09T03:00:00Z"))
        assertEquals(LocalDate.parse("2026-09-09"), today)
        assertEquals("Today", articleDayLabel(today, today, Locale.US))
        assertEquals("Yesterday", articleDayLabel(today.minusDays(1), today, Locale.US))
        assertEquals("Monday, Sep 7", articleDayLabel(today.minusDays(2), today, Locale.US))
    }
}
