package dev.carraes.lam.ui.history

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.components.*
import dev.carraes.lam.ui.markdown.*
import dev.carraes.lam.ui.theme.Graphite
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HistoryScreen(state: HistoryState, onQuery: (String) -> Unit, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClearFilters: () -> Unit, onRefresh: () -> Unit, onMore: () -> Unit,
    onRequests: () -> Unit, onSettings: () -> Unit) {
    var search by rememberSaveable { mutableStateOf(false) }
    var filters by rememberSaveable { mutableStateOf(false) }
    var destination by rememberSaveable { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding()) {
            QueueTopBar(stringResource(R.string.history_title), onRequests, {}, onSettings, { search = !search },
                searchLabel = stringResource(R.string.search_history))
            if (search || state.query.query != null) OutlinedTextField(state.query.query.orEmpty(), onQuery,
                label = { Text(stringResource(R.string.search_hint)) }, singleLine = true,
                trailingIcon = { IconButton({ if (state.query.query != null) onQuery("") else search = false }) {
                    Icon(Icons.Default.Close, stringResource(if (state.query.query != null) R.string.clear_search else R.string.close_search))
                } }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(pluralStringResource(R.plurals.history_count, state.items.size, state.items.size), Modifier.weight(1f))
                val count = listOfNotNull(state.query.priority, state.query.type).size
                TextButton({ filters = true }) { Text(stringResource(if (count == 0) R.string.filters else R.string.filters_active, count)) }
                TextButton(onRefresh, enabled = !state.loading) { Text(stringResource(R.string.detail_refresh)) }
            }
            PullToRefreshBox(state.loading, onRefresh, Modifier.weight(1f)) {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    if (state.stale) item {
                        Text(stringResource(R.string.history_offline))
                        Text(state.lastSuccess?.let { stringResource(R.string.requests_last_success, historyTime(it.toString())) }
                            ?: stringResource(R.string.requests_never_synced), style = MaterialTheme.typography.bodySmall)
                    }
                    if (state.loadFailed) item { Text(stringResource(R.string.history_failed)) }
                    if (state.incomplete) item { Text(stringResource(R.string.history_incomplete)) }
                    if (state.items.isEmpty() && !state.loading) item { Text(stringResource(R.string.history_empty)) }
                    items(state.items, key = { it.id }) { row ->
                        Card(Modifier.fillMaxWidth().testTag("history:${row.id}")) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(row.title, style = MaterialTheme.typography.titleMedium)
                                Text(row.agentDisplay, style = MaterialTheme.typography.labelLarge)
                                val status = stringResource(when (row.status) {
                                    StatusDto.RESOLVED -> R.string.detail_resolved
                                    StatusDto.DISMISSED -> R.string.detail_dismissed
                                    StatusDto.RETRACTED -> R.string.detail_retracted
                                    StatusDto.EXPIRED -> R.string.detail_expired
                                    StatusDto.OPEN -> R.string.request_open
                                })
                                Text(row.responseBy?.let { stringResource(R.string.detail_outcome_via, status,
                                    if (it == ResponseByDto.CLI) "CLI" else stringResource(R.string.detail_phone)) } ?: status)
                                row.responseChoice?.let { Text(it) }
                                row.responseText?.let { Text(it) }
                                row.effectiveClosureTime?.let { Text(historyTime(it), style = MaterialTheme.typography.bodySmall) }
                                if (row.body.isNotBlank()) LamMarkdown(row.body, { destination = it })
                                row.checks.forEach { check -> Text(stringResource(if (check.done) R.string.history_check_done else R.string.history_check_pending, check.label)) }
                            }
                        }
                    }
                    if (state.nextCursor != null) item {
                        TextButton(onMore, enabled = !state.loading) { Text(stringResource(R.string.history_more)) }
                    }
                }
            }
        }
    }
    if (filters) FilterSheet(state.query.type, state.query.priority, onType, onPriority, onClearFilters, { filters = false })
    destination?.let { LinkDestinationDialog(it, { destination = null }, { url -> openWebLink(context, url) }) }
}

private fun historyTime(value: String): String = runCatching {
    DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(Instant.parse(value))
}.getOrDefault(value)
