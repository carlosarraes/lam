package dev.carraes.lam.ui.components

import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R

val LocalArticlesNavigation = staticCompositionLocalOf<(() -> Unit)?> { null }

@Composable
fun QueueTopBar(title: String, onRequests: () -> Unit, onHistory: () -> Unit, onSettings: () -> Unit, onSearch: (() -> Unit)? = null,
    searchLabel: String = stringResource(R.string.search_requests)) {
    var queueMenu by remember { mutableStateOf(false) }
    var overflow by remember { mutableStateOf(false) }
    val switchLabel = stringResource(R.string.switch_queue)
    Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Box {
            TextButton(onClick = { queueMenu = true }, modifier = Modifier.semantics { contentDescription = switchLabel },
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.onBackground)) {
                Text(title, style = MaterialTheme.typography.headlineMedium)
                Icon(Icons.Default.ArrowDropDown, contentDescription = null)
            }
            DropdownMenu(queueMenu, onDismissRequest = { queueMenu = false }) {
                DropdownMenuItem(text = { Text(stringResource(R.string.requests_title)) }, onClick = { queueMenu = false; onRequests() })
                DropdownMenuItem(text = { Text(stringResource(R.string.history_title)) }, onClick = { queueMenu = false; onHistory() })
                LocalArticlesNavigation.current?.let { onArticles ->
                    DropdownMenuItem(text = { Text(stringResource(R.string.articles_title)) }, onClick = { queueMenu = false; onArticles() })
                }
            }
        }
        if (onSearch != null) IconButton(onSearch) { Icon(Icons.Default.Search, searchLabel) }
        Spacer(Modifier.weight(1f))
        Box {
            IconButton(onClick = { overflow = true }) { Icon(Icons.Default.MoreVert, stringResource(R.string.more_options)) }
            DropdownMenu(overflow, onDismissRequest = { overflow = false }) {
                DropdownMenuItem(text = { Text(stringResource(R.string.settings_title)) }, onClick = { overflow = false; onSettings() })
            }
        }
    }
}
