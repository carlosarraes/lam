package dev.carraes.lam.ui.requests

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.semantics.*
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.components.*
import dev.carraes.lam.ui.theme.Amber
import dev.carraes.lam.ui.theme.Graphite
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RequestsScreen(state: RequestsState, onQuery: (String) -> Unit, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClearFilters: () -> Unit, onRefresh: () -> Unit,
    onHistory: () -> Unit, onSettings: () -> Unit, onItem: (String) -> Unit,
    onQuickResponse: (String) -> Unit = {}, onDismiss: (String) -> Unit = {}) {
    var search by rememberSaveable { mutableStateOf(false) }
    var filters by rememberSaveable { mutableStateOf(false) }
    val activeFilters = listOfNotNull(state.type, state.priority).size
    val filtered = activeFilters > 0 || state.query.isNotBlank()
    val refreshLabel = stringResource(R.string.requests_refresh)
    val filterLabel = stringResource(R.string.filters)
    Surface(Modifier.fillMaxSize().testTag("requests-surface"), color = Graphite) {
        Column(Modifier.safeDrawingPadding()) {
            QueueTopBar(stringResource(R.string.requests_title), {}, onHistory, onSettings, { search = !search })
            if (search || state.query.isNotEmpty()) {
                OutlinedTextField(value = state.query, onValueChange = onQuery, singleLine = true,
                    label = { Text(stringResource(R.string.search_hint)) }, shape = RoundedCornerShape(8.dp),
                    trailingIcon = { IconButton(onClick = { if (state.query.isNotEmpty()) onQuery("") else search = false }) {
                        Icon(Icons.Default.Close, stringResource(if (state.query.isNotEmpty()) R.string.clear_search else R.string.close_search))
                    } }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(if (filtered) R.string.requests_matching_counts else R.string.requests_pending_counts,
                    pluralStringResource(R.plurals.requests_count, state.actionableCount, state.actionableCount),
                    pluralStringResource(R.plurals.requests_fyi_count, state.informationalCount, state.informationalCount)),
                    style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.weight(1f))
                TextButton(onClick = { filters = true }, modifier = Modifier.semantics { contentDescription = filterLabel }) {
                    Text(if (activeFilters == 0) filterLabel else stringResource(R.string.filters_active, activeFilters))
                }
            }
            PullToRefreshBox(isRefreshing = state.refreshing, onRefresh = onRefresh, modifier = Modifier.weight(1f).semantics {
                customActions = listOf(CustomAccessibilityAction(refreshLabel) { onRefresh(); true })
            }) {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (state.stale) item(key = "stale") {
                        Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
                            Text(stringResource(R.string.requests_offline), color = Amber, style = MaterialTheme.typography.labelLarge)
                            Text(state.lastSuccess?.let {
                                stringResource(R.string.requests_last_success, DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT)
                                    .withZone(ZoneId.systemDefault()).format(it))
                            } ?: stringResource(R.string.requests_never_synced), style = MaterialTheme.typography.bodySmall)
                            TextButton(onRefresh) { Text(stringResource(R.string.pairing_retry_sync)) }
                        }
                    }
                    if (state.items.isEmpty()) item(key = "empty") {
                        Column(Modifier.fillMaxWidth().padding(top = 60.dp, bottom = 32.dp), horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            if (state.loading) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                            Text(stringResource(when {
                                state.loading -> R.string.requests_loading
                                filtered -> R.string.requests_no_matches
                                state.stale -> R.string.requests_error
                                else -> R.string.requests_empty
                            }), style = MaterialTheme.typography.titleMedium)
                            if (!state.loading && !state.stale) Text(stringResource(if (filtered) R.string.requests_no_matches_hint else R.string.requests_empty_hint),
                                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                    items(state.items, key = { "request:${it.id}" }) { item ->
                        RequestCard(item, state.now, actionsEnabled = state.actionsEnabled,
                            onQuickResponse = { onQuickResponse(item.id) }, onDismiss = { onDismiss(item.id) }) { onItem(item.id) }
                    }
                }
            }
        }
    }
    if (filters) FilterSheet(state.type, state.priority, onType, onPriority, onClearFilters, { filters = false })
}
