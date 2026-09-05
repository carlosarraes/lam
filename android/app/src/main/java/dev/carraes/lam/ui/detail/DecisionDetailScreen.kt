package dev.carraes.lam.ui.detail

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.carraes.lam.R
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.markdown.*
import dev.carraes.lam.ui.theme.*
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@Composable
fun DecisionDetailScreen(viewModel: DecisionViewModel, onBack: () -> Unit) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    DecisionDetailScreen(state, onBack, viewModel::refresh, viewModel::choose, viewModel::writeReply,
        viewModel::editReply, viewModel::reviewReply, viewModel::dismissReply, viewModel::confirm, viewModel::dismissConfirmation)
}

@Composable
fun DecisionDetailScreen(
    state: DecisionState,
    onBack: () -> Unit,
    onRefresh: () -> Unit = {},
    onChoice: (String) -> Unit = {},
    onWriteReply: () -> Unit = {},
    onEditReply: (String) -> Unit = {},
    onReviewReply: () -> Unit = {},
    onDismissReply: () -> Unit = {},
    onConfirm: (AnswerConfirmation) -> Unit = {},
    onDismissConfirmation: () -> Unit = {},
    onOpenLink: ((String) -> Boolean)? = null,
) {
    val context = LocalContext.current
    var destination by rememberSaveable(state.item?.id) { mutableStateOf<String?>(null) }
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
                Text(stringResource(R.string.request_detail), style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                TextButton(onRefresh, enabled = !state.refreshing && !state.submitting) { Text(stringResource(R.string.detail_refresh)) }
            }
            Column(Modifier.fillMaxWidth().weight(1f).verticalScroll(rememberScrollState()).padding(20.dp).testTag("detail-scroll"),
                verticalArrangement = Arrangement.spacedBy(20.dp)) {
                val item = state.item
                if (item == null) {
                    Text(stringResource(when {
                        state.loading -> R.string.detail_loading
                        state.loadFailed -> R.string.detail_load_failed
                        else -> R.string.detail_missing
                    }))
                    if (!state.loading) TextButton(onRefresh, enabled = !state.refreshing) { Text(stringResource(R.string.pairing_retry_sync)) }
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(item.title, style = MaterialTheme.typography.headlineMedium, modifier = Modifier.semantics { heading() })
                        Text(stringResource(when (item.priority) {
                            PriorityDto.CRITICAL -> R.string.priority_critical
                            PriorityDto.NORMAL -> R.string.priority_normal
                            PriorityDto.LOW -> R.string.priority_low
                        }), color = if (item.priority == PriorityDto.CRITICAL) Amber else MutedText, style = MaterialTheme.typography.labelLarge)
                        Text(item.agentDisplay, style = MaterialTheme.typography.titleSmall)
                        Text("${item.sourceHost}:${item.sourceProject}", color = MutedText, style = MaterialTheme.typography.bodySmall)
                        Text(formatTime(item.createdAt), color = MutedText, style = MaterialTheme.typography.bodySmall)
                    }
                    RecommendationCard(item)
                    LamMarkdown(item.body, onLink = { destination = it }, modifier = Modifier.testTag("markdown-body"))
                    if (item.link.isNotBlank()) TextButton(onClick = { destination = item.link }) { Text(stringResource(R.string.detail_related_link)) }
                    if (item.status != StatusDto.OPEN) {
                        CanonicalOutcome(item)
                    } else {
                        if (!state.sync.mutationsEnabled || state.loadFailed) {
                            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                Text(stringResource(if (state.sync is SyncState.Stale) R.string.requests_offline else R.string.detail_reconciling), color = Amber)
                                val lastSuccess = (state.sync as? SyncState.Stale)?.lastSuccess
                                if (lastSuccess != null) Text(stringResource(R.string.requests_last_success, formatTime(lastSuccess.toString())), style = MaterialTheme.typography.bodySmall)
                                TextButton(onRefresh, enabled = !state.refreshing) { Text(stringResource(R.string.pairing_retry_sync)) }
                            }
                        }
                        if (state.answerFailed) Text(stringResource(R.string.detail_answer_failed), color = Amber)
                        if (state.submitting) Text(stringResource(R.string.detail_sending), modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite })
                        if (item.checks.isNotEmpty()) {
                            Text(stringResource(R.string.detail_checklist_read_only), color = MutedText)
                            item.checks.forEach { check -> Text(stringResource(if (check.done) R.string.detail_check_done else R.string.detail_check_pending, check.label)) }
                        } else {
                            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                item.choices.forEachIndexed { index, choice ->
                                    val recommended = choice == item.recommendedChoice
                                    OutlinedButton(onClick = { onChoice(choice) }, enabled = state.actionsEnabled,
                                        border = BorderStroke(1.dp, if (recommended) Amber.copy(alpha = 0.65f) else MaterialTheme.colorScheme.outline),
                                        shape = RoundedCornerShape(8.dp), modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp).testTag("choice-$index")) {
                                        Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                            Text(choice)
                                            if (recommended) Text(stringResource(R.string.detail_recommended), color = Amber, style = MaterialTheme.typography.labelSmall)
                                        }
                                    }
                                }
                                TextButton(onWriteReply, enabled = state.actionsEnabled, modifier = Modifier.fillMaxWidth()) { Text(stringResource(R.string.detail_write_reply)) }
                            }
                        }
                    }
                }
            }
        }
    }
    val confirmation = state.confirmation
    if (confirmation != null) {
        AnswerConfirmationSheet(confirmation, state.actionsEnabled, { onConfirm(confirmation) }, onDismissConfirmation)
    } else if (state.replyOpen) ReplySheet(state, onEditReply, onReviewReply, onDismissReply)
    destination?.let { link -> LinkDestinationDialog(link, { destination = null }, onOpenLink ?: { openWebLink(context, it) }) }
}

@Composable
private fun CanonicalOutcome(item: Item) {
    Column(Modifier.fillMaxWidth().testTag("outcome").semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
        verticalArrangement = Arrangement.spacedBy(8.dp)) {
        val status = stringResource(when (item.status) {
            StatusDto.RESOLVED -> R.string.detail_resolved
            StatusDto.DISMISSED -> R.string.detail_dismissed
            StatusDto.RETRACTED -> R.string.detail_retracted
            StatusDto.EXPIRED -> R.string.detail_expired
            StatusDto.OPEN -> R.string.request_open
        })
        Text(item.responseBy?.let { stringResource(R.string.detail_outcome_via, status, if (it == ResponseByDto.CLI) "CLI" else stringResource(R.string.detail_phone)) } ?: status,
            style = MaterialTheme.typography.titleMedium)
        item.responseChoice?.let { Text(it) }
        item.responseText?.let { Text(it) }
        item.resolvedAt?.let { Text(formatTime(it), color = MutedText, style = MaterialTheme.typography.bodySmall) }
    }
}

private fun formatTime(value: String): String = runCatching {
    DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(Instant.parse(value))
}.getOrDefault(value)
