package dev.carraes.lam.articles

import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

private val articleZone = ZoneId.of("America/Sao_Paulo")

fun articleToday(now: Instant = Instant.now()): LocalDate = now.atZone(articleZone).toLocalDate()

fun articleDayLabel(day: LocalDate, today: LocalDate, locale: Locale = Locale.getDefault()): String = when (day) {
    today -> "Today"
    today.minusDays(1) -> "Yesterday"
    else -> day.format(DateTimeFormatter.ofPattern(if (day.year == today.year) "EEEE, MMM d" else "EEEE, MMM d, yyyy", locale))
}

internal fun articleOnDay(createdAt: String, day: String): Boolean =
    runCatching { articleToday(Instant.parse(createdAt)).toString() == day }.getOrDefault(false)

internal fun articleTime(value: String, locale: Locale = Locale.getDefault()): String = runCatching {
    DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT).withLocale(locale).withZone(articleZone).format(Instant.parse(value))
}.getOrDefault(value)
