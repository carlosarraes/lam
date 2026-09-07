package dev.carraes.lam.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.*

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun FilterSheet(type: ItemTypeDto?, priority: PriorityDto?, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClear: () -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.filters), style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                TextButton(onDismiss) { Text(stringResource(R.string.done)) }
            }
            Text(stringResource(R.string.filter_type), style = MaterialTheme.typography.labelLarge)
            Column(Modifier.selectableGroup()) {
                FilterOption(stringResource(R.string.all_types), type == null) { onType(null) }
                ItemTypeDto.entries.forEach { value -> FilterOption(typeLabel(value), type == value) { onType(value) } }
            }
            HorizontalDivider(Modifier.padding(vertical = 8.dp))
            Text(stringResource(R.string.filter_priority), style = MaterialTheme.typography.labelLarge)
            Column(Modifier.selectableGroup()) {
                FilterOption(stringResource(R.string.all_priorities), priority == null) { onPriority(null) }
                listOf(PriorityDto.CRITICAL, PriorityDto.NORMAL, PriorityDto.LOW).forEach { value ->
                    FilterOption(priorityLabel(value), priority == value) { onPriority(value) }
                }
            }
            TextButton(onClear) { Text(stringResource(R.string.clear_filters)) }
            Spacer(Modifier.height(16.dp))
        }
    }
}

@Composable
private fun FilterOption(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).selectable(selected, role = Role.RadioButton, onClick = onClick),
        verticalAlignment = Alignment.CenterVertically) {
        RadioButton(selected, onClick = null)
        Text(label, modifier = Modifier.padding(start = 12.dp))
    }
}

@Composable
fun priorityLabel(priority: PriorityDto, kind: ItemKindDto = ItemKindDto.REQUEST): String = stringResource(when (priority) {
    PriorityDto.CRITICAL -> R.string.priority_critical
    PriorityDto.NORMAL -> if (kind == ItemKindDto.FYI) R.string.priority_fyi_normal else R.string.priority_normal
    PriorityDto.LOW -> R.string.priority_low
})

@Composable
fun typeLabel(type: ItemTypeDto): String = stringResource(when (type) {
    ItemTypeDto.PLAIN -> R.string.type_plain
    ItemTypeDto.CHOICE -> R.string.type_choice
    ItemTypeDto.CHECKLIST -> R.string.type_checklist
})
