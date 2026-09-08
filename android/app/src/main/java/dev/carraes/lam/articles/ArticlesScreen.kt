package dev.carraes.lam.articles

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.ui.components.QueueTopBar
import dev.carraes.lam.ui.theme.Graphite
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ArticlesScreen(state: ArticlesState, onQuery: (String) -> Unit, onFilter: (String) -> Unit, onRefresh: () -> Unit,
    onMore: () -> Unit, onArticle: (String) -> Unit, onUnread: (Article) -> Unit,
    onRequests: () -> Unit, onHistory: () -> Unit, onSettings: () -> Unit) {
    var search by rememberSaveable { mutableStateOf(false) }
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding()) {
            QueueTopBar(stringResource(R.string.articles_title), onRequests, onHistory, onSettings, { search = !search },
                searchLabel = stringResource(R.string.articles_search))
            if (search || state.query.isNotEmpty()) OutlinedTextField(state.query, onQuery, singleLine = true,
                label = { Text(stringResource(R.string.articles_search)) }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("all" to R.string.articles_all, "unread" to R.string.articles_unread, "read" to R.string.articles_read).forEach { (filter, label) ->
                    FilterChip(state.readFilter == filter, { onFilter(filter) }, { Text(stringResource(label)) })
                }
                Spacer(Modifier.weight(1f))
                TextButton(onRefresh, enabled = !state.loading) { Text(stringResource(R.string.detail_refresh)) }
            }
            PullToRefreshBox(state.loading, onRefresh, Modifier.weight(1f)) {
                LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (state.failed) item { Text(stringResource(if (state.cached) R.string.articles_cached_list else R.string.articles_failed)) }
                    if (state.items.isEmpty() && !state.loading) item { Text(stringResource(R.string.articles_empty)) }
                    items(state.items, key = { it.id }) { article ->
                        Card(onClick = { onArticle(article.id) }, modifier = Modifier.fillMaxWidth().testTag("article:${article.id}")) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(article.title, style = MaterialTheme.typography.titleMedium)
                                Text(article.summary, style = MaterialTheme.typography.bodyMedium)
                                Text(article.name, style = MaterialTheme.typography.labelLarge)
                                Text("${article.sourceHost} · ${article.sourceProject}", style = MaterialTheme.typography.bodySmall)
                                Text(articleTime(article.createdAt), style = MaterialTheme.typography.bodySmall)
                                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    Text(stringResource(if (article.readAt == null) R.string.articles_unread else R.string.articles_read),
                                        style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                                    if (article.readAt != null) TextButton({ onUnread(article) }) { Text(stringResource(R.string.articles_mark_unread)) }
                                }
                            }
                        }
                    }
                    if (state.nextCursor != null) item { TextButton(onMore, enabled = !state.loading) { Text(stringResource(R.string.history_more)) } }
                }
            }
        }
    }
}

private fun articleTime(value: String) = runCatching {
    DateTimeFormatter.ofLocalizedDateTime(FormatStyle.SHORT).withZone(ZoneId.systemDefault()).format(Instant.parse(value))
}.getOrDefault(value)
