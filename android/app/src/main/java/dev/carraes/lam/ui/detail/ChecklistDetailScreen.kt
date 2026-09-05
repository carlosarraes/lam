package dev.carraes.lam.ui.detail

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.CheckDto

@Composable
fun ChecklistDetailScreen(checks: List<CheckDto>, enabled: Boolean, onCheck: (Int, Boolean) -> Unit,
    onReply: () -> Unit, remainingOnly: Boolean = false) {
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        checks.forEachIndexed { index, check ->
            key(index) {
                if (!remainingOnly || !check.done) {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp)
                        .testTag(if (remainingOnly) "quick-check-$index" else "check-$index")
                        .toggleable(check.done, enabled = enabled, role = Role.Checkbox, onValueChange = { onCheck(index, it) })
                        .padding(horizontal = 4.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(check.done, onCheckedChange = null, enabled = enabled)
                        Text(check.label, modifier = Modifier.padding(start = 12.dp).weight(1f))
                    }
                }
            }
        }
        TextButton(onReply, enabled = enabled, modifier = Modifier.fillMaxWidth().testTag(if (remainingOnly) "quick-reply" else "checklist-reply")) {
            Text(stringResource(R.string.reply_instead))
        }
    }
}
