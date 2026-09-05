package dev.carraes.lam.ui.requests

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DismissConfirmation(enabled: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)) {
        Column(Modifier.fillMaxWidth().padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(stringResource(R.string.dismiss_confirm), style = MaterialTheme.typography.titleLarge)
            Text(stringResource(R.string.dismiss_explanation))
            Button(onConfirm, enabled = enabled, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.dismiss_request)) }
            TextButton(onDismiss, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.pairing_cancel)) }
        }
    }
}
