package dev.carraes.lam.articles

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowLeft
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
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
import java.time.LocalDate
import java.time.ZoneOffset

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ArticlesScreen(state: ArticlesState, onQuery: (String) -> Unit, onFilter: (String) -> Unit, onRefresh: () -> Unit,
    onMore: () -> Unit, onArticle: (String) -> Unit, onUnread: (Article) -> Unit,
    onRequests: () -> Unit, onHistory: () -> Unit, onSettings: () -> Unit,
    onDay: (String?) -> Unit = {}, onAllUnread: () -> Unit = {}) {
    var search by rememberSaveable { mutableStateOf(false) }
    var datePicker by rememberSaveable { mutableStateOf(false) }
    var today by remember { mutableStateOf(articleToday()) }
    LaunchedEffect(Unit) {
        while (true) {
            today = articleToday()
            kotlinx.coroutines.delay(1_000)
        }
    }
    val selected = state.day?.let(LocalDate::parse)
    if (datePicker) {
        val picker = rememberDatePickerState(
            initialSelectedDateMillis = (selected ?: today).atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli(),
            selectableDates = object : SelectableDates {
                override fun isSelectableDate(utcTimeMillis: Long) =
                    !Instant.ofEpochMilli(utcTimeMillis).atZone(ZoneOffset.UTC).toLocalDate().isAfter(today)
                override fun isSelectableYear(year: Int) = year <= today.year
            })
        DatePickerDialog(onDismissRequest = { datePicker = false }, confirmButton = {
            TextButton({
                picker.selectedDateMillis?.let { onDay(Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate().toString()) }
                datePicker = false
            }, enabled = picker.selectedDateMillis != null) { Text("OK") }
        }, dismissButton = { TextButton({ datePicker = false }) { Text("Cancel") } }) {
            DatePicker(picker)
        }
    }
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding()) {
            QueueTopBar(stringResource(R.string.articles_title), onRequests, onHistory, onSettings, { search = !search },
                searchLabel = stringResource(R.string.articles_search))
            if (search || state.query.isNotEmpty()) OutlinedTextField(state.query, onQuery, singleLine = true,
                label = { Text(stringResource(R.string.articles_search)) }, modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp))
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                IconButton({ onDay((selected ?: today).minusDays(1).toString()) }, enabled = selected != null) {
                    Icon(Icons.AutoMirrored.Filled.KeyboardArrowLeft, "Previous day")
                }
                TextButton({ datePicker = true }, Modifier.weight(1f)) {
                    Text(selected?.let { articleDayLabel(it, today) } ?: "All dates")
                }
                IconButton({ onDay(selected?.plusDays(1)?.toString()) }, enabled = selected != null && selected < today) {
                    Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, "Next day")
                }
                if (selected != today) TextButton({ onDay(today.toString()) }) { Text("Today") }
            }
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf("all" to R.string.articles_all, "unread" to R.string.articles_unread, "read" to R.string.articles_read).forEach { (filter, label) ->
                    FilterChip(state.day != null && state.readFilter == filter, { onFilter(filter) }, { Text(stringResource(label)) })
                }
                FilterChip(state.day == null && state.readFilter == "unread", onAllUnread, { Text("All unread") })
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
