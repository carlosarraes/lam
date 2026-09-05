package dev.carraes.lam.ui.requests

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.Item
import dev.carraes.lam.ui.agentColor
import dev.carraes.lam.ui.components.priorityLabel
import dev.carraes.lam.ui.theme.Amber
import dev.carraes.lam.ui.theme.OffWhite
import java.time.Instant

@Composable
fun RequestCard(item: Item, now: Instant, onOpen: () -> Unit) {
    val priority = priorityLabel(item.priority)
    val progress = when {
        item.checks.isNotEmpty() -> pluralStringResource(R.plurals.request_check_progress, item.checks.size, item.checks.count { it.done }, item.checks.size)
        item.choices.isNotEmpty() -> pluralStringResource(R.plurals.request_choice_count, item.choices.size, item.choices.size)
        else -> stringResource(R.string.request_decision)
    }
    val type = stringResource(if (item.checks.isNotEmpty()) R.string.type_checklist else R.string.request_decision)
    val warning = stringResource(R.string.request_no_recommendation)
    val open = stringResource(R.string.request_open)
    val details = listOf(type, progress).distinct().joinToString(", ") + if (item.missingRecommendation) ", $warning" else ""
    val description = stringResource(R.string.request_accessibility, item.title, item.agentDisplay, priority, details, open)
    Card(onClick = onOpen, shape = RoundedCornerShape(8.dp),
        colors = CardDefaults.cardColors(containerColor = agentColor(item), contentColor = OffWhite),
        modifier = Modifier.fillMaxWidth().clearAndSetSemantics {
            contentDescription = description
            role = Role.Button
            onClick(label = open) { onOpen(); true }
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
}
