package dev.carraes.lam.ui

import androidx.compose.ui.graphics.Color
import dev.carraes.lam.items.Item
import dev.carraes.lam.items.PriorityDto
import dev.carraes.lam.ui.theme.MutedBurgundy

private val agentPalette = listOf(
    Color(0xFF303E42), Color(0xFF3B414D), Color(0xFF3F3A48),
    Color(0xFF39453D), Color(0xFF4B4338), Color(0xFF39454C),
)

fun agentColor(item: Item): Color {
    if (item.priority == PriorityDto.CRITICAL) return MutedBurgundy
    // JVM String.hashCode is specified and stable across launches and devices.
    return agentPalette[Math.floorMod(item.agentDisplay.hashCode(), agentPalette.size)]
}
