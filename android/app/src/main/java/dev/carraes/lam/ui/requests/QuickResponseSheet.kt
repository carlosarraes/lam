package dev.carraes.lam.ui.requests

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.ui.detail.*
import dev.carraes.lam.ui.theme.Amber

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun QuickResponseSheet(state: DecisionState, checksEnabled: Boolean, onChoice: (String) -> Unit,
    onCheck: (Int, Boolean) -> Unit, onReply: () -> Unit, onDismiss: () -> Unit, snackbar: SnackbarHostState? = null) {
    val item = state.item ?: return
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.quick_response), style = MaterialTheme.typography.titleLarge)
            Text(item.title, style = MaterialTheme.typography.titleMedium)
            if (item.checks.isNotEmpty()) {
                ChecklistDetailScreen(item.checks, state.actionsEnabled && checksEnabled, onCheck, onReply, remainingOnly = true)
            } else {
                item.choices.forEachIndexed { index, choice ->
                    val recommended = choice == item.recommendedChoice
                    OutlinedButton({ onChoice(choice) }, enabled = state.actionsEnabled,
                        border = BorderStroke(1.dp, if (recommended) Amber else MaterialTheme.colorScheme.outline),
                        modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("quick-choice-$index")) {
                        Column(Modifier.fillMaxWidth()) {
                            Text(choice)
                            if (recommended) Text(stringResource(R.string.detail_recommended), color = Amber, style = MaterialTheme.typography.labelSmall)
                        }
                    }
                }
                TextButton(onReply, enabled = state.actionsEnabled, modifier = Modifier.fillMaxWidth().testTag("quick-reply")) { Text(stringResource(R.string.detail_write_reply)) }
            }
            if (snackbar != null) SnackbarHost(snackbar)
        }
    }
}
