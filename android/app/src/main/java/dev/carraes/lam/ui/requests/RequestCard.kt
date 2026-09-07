package dev.carraes.lam.ui.requests

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.IntOffset
import dev.carraes.lam.R
import dev.carraes.lam.items.Item
import dev.carraes.lam.ui.agentColor
import dev.carraes.lam.ui.components.priorityLabel
import dev.carraes.lam.ui.theme.Amber
import dev.carraes.lam.ui.theme.OffWhite
import java.time.Instant
import kotlin.math.roundToInt

@Composable
fun RequestCard(item: Item, now: Instant, actionsEnabled: Boolean = false,
    onQuickResponse: () -> Unit = {}, onDismiss: () -> Unit = {}, onOpen: () -> Unit) {
    val priority = priorityLabel(item.priority, item.kind)
    val progress = when {
        item.isFyi -> stringResource(R.string.item_fyi)
        item.checks.isNotEmpty() -> pluralStringResource(R.plurals.request_check_progress, item.checks.size, item.checks.count { it.done }, item.checks.size)
        item.choices.isNotEmpty() -> pluralStringResource(R.plurals.request_choice_count, item.choices.size, item.choices.size)
        else -> stringResource(R.string.request_decision)
    }
    val type = stringResource(if (item.isFyi) R.string.item_fyi else if (item.checks.isNotEmpty()) R.string.type_checklist else R.string.request_decision)
    val warning = stringResource(R.string.request_no_recommendation)
    val open = stringResource(if (item.isFyi) R.string.fyi_open else R.string.request_open)
    val details = listOf(type, progress).distinct().joinToString(", ") + if (item.missingRecommendation) ", $warning" else ""
    val description = stringResource(R.string.request_accessibility, item.title, item.agentDisplay, priority, details, open)
    val quick = stringResource(R.string.quick_response)
    val dismiss = stringResource(R.string.dismiss)
    var menu by remember(item.id) { mutableStateOf(false) }
    var drag by remember(item.id) { mutableFloatStateOf(0f) }
    val threshold = with(LocalDensity.current) { 88.dp.toPx() }
    val currentQuick by rememberUpdatedState(onQuickResponse)
    val currentDismiss by rememberUpdatedState(onDismiss)
    LaunchedEffect(actionsEnabled) { if (!actionsEnabled) { menu = false; drag = 0f } }
    Box(Modifier.fillMaxWidth()) {
        if (drag != 0f) Row(Modifier.matchParentSize().background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(8.dp)).padding(20.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.SpaceBetween) {
            // Keep the physical sides fixed even when the surrounding layout is RTL.
            CompositionLocalProvider(androidx.compose.ui.platform.LocalLayoutDirection provides androidx.compose.ui.unit.LayoutDirection.Ltr) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Icon(Icons.Default.Close, dismiss)
                    if (!item.isFyi) Icon(Icons.AutoMirrored.Filled.Send, quick)
                }
            }
        }
    Card(shape = RoundedCornerShape(8.dp),
        colors = CardDefaults.cardColors(containerColor = agentColor(item), contentColor = OffWhite),
        modifier = Modifier.fillMaxWidth().absoluteOffset { IntOffset(drag.roundToInt(), 0) }
            .testTag("request-card-${item.id}")
            .pointerInput(item.id, item.isFyi, actionsEnabled, threshold) {
                detectHorizontalDragGestures(
                    onHorizontalDrag = { change, amount ->
                        // Consume stale-card swipes too, so releasing in bounds cannot open detail.
                        change.consume()
                        if (actionsEnabled) drag = (drag + amount).coerceIn(if (item.isFyi) 0f else -threshold * 1.5f, threshold * 1.5f)
                    },
                    onDragEnd = {
                        val completed = drag
                        drag = 0f
                        if (actionsEnabled) {
                            if (completed >= threshold) currentDismiss() else if (!item.isFyi && completed <= -threshold) currentQuick()
                        }
                    },
                    onDragCancel = { drag = 0f },
                )
            }
            .combinedClickable(onClick = onOpen, onLongClick = if (actionsEnabled) ({ menu = true }) else null)
            .clearAndSetSemantics {
            contentDescription = description
            role = Role.Button
            onClick(label = open) { onOpen(); true }
            if (actionsEnabled) {
                onLongClick(label = if (item.isFyi) dismiss else quick) { menu = true; true }
                customActions = buildList {
                    if (!item.isFyi) add(CustomAccessibilityAction(quick) { onQuickResponse(); true })
                    add(CustomAccessibilityAction(dismiss) { onDismiss(); true })
                }
            }
        }) {
        Column(Modifier.padding(18.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(item.title, style = MaterialTheme.typography.titleLarge)
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(item.agentDisplay, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.weight(1f))
                Spacer(Modifier.width(12.dp))
                Text(requestAge(item, now), style = MaterialTheme.typography.labelMedium)
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                Text(priority, style = MaterialTheme.typography.labelLarge)
                Text(progress, style = MaterialTheme.typography.labelLarge)
            }
            if (item.missingRecommendation) Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Spacer(Modifier.width(2.dp).height(12.dp).background(Amber))
                Text(warning, style = MaterialTheme.typography.labelMedium)
            }
        }
    }
        DropdownMenu(menu, onDismissRequest = { menu = false }) {
            if (!item.isFyi) DropdownMenuItem(text = { Text(quick) }, onClick = { menu = false; onQuickResponse() })
            DropdownMenuItem(text = { Text(dismiss) }, onClick = { menu = false; onDismiss() })
        }
    }
}
