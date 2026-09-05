package dev.carraes.lam.ui.detail

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.FinalAnswer

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ReplySheet(state: DecisionState, onEdit: (String) -> Unit, onReview: () -> Unit, onDismiss: () -> Unit) {
    val focus = remember { FocusRequester() }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)) {
        Column(Modifier.fillMaxWidth().imePadding().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.detail_write_reply), style = MaterialTheme.typography.titleLarge)
            Text(stringResource(R.string.detail_reply_resolves), style = MaterialTheme.typography.bodyMedium)
            OutlinedTextField(state.reply, onEdit, modifier = Modifier.fillMaxWidth().testTag("reply-input").focusRequester(focus),
                label = { Text(stringResource(R.string.detail_reply_label)) }, minLines = 4, maxLines = 10,
                enabled = !state.submitting, isError = state.replyBytes > 8192, shape = RoundedCornerShape(8.dp),
                supportingText = { Text(stringResource(R.string.detail_reply_bytes, state.replyBytes)) })
            Button(onReview, enabled = state.actionsEnabled && state.replyValid, shape = RoundedCornerShape(8.dp), modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.detail_review_reply))
            }
        }
        LaunchedEffect(Unit) { focus.requestFocus() }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AnswerConfirmationSheet(confirmation: AnswerConfirmation, enabled: Boolean, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), shape = RoundedCornerShape(topStart = 12.dp, topEnd = 12.dp)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            Text(when (val answer = confirmation.answer) {
                is FinalAnswer.Choice -> stringResource(R.string.detail_confirm_choice, answer.value)
                else -> stringResource(R.string.detail_confirm_reply)
            }, style = MaterialTheme.typography.titleLarge)
            if (confirmation.answer is FinalAnswer.Text) Text(confirmation.answer.value)
            Text(stringResource(R.string.detail_reply_resolves), style = MaterialTheme.typography.bodyMedium)
            Button(onConfirm, enabled = enabled, modifier = Modifier.fillMaxWidth(), shape = RoundedCornerShape(8.dp)) { Text(stringResource(R.string.detail_send_reply)) }
            TextButton(onDismiss, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.pairing_cancel)) }
        }
    }
}
