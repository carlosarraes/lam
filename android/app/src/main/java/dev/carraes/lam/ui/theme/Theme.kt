package dev.carraes.lam.ui.theme

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable

private val LamDarkColors = darkColorScheme(
    primary = Amber,
    onPrimary = Graphite,
    secondary = Amber,
    background = Graphite,
    onBackground = OffWhite,
    surface = GraphiteRaised,
    onSurface = OffWhite,
    onSurfaceVariant = MutedText,
    errorContainer = MutedBurgundy,
    onErrorContainer = OffWhite,
)

@Composable
fun LamTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = LamDarkColors,
        typography = LamTypography,
        content = content,
    )
}
