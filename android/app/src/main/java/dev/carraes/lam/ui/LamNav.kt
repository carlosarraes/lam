package dev.carraes.lam.ui

import android.net.Uri
import androidx.compose.foundation.layout.*
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.navigation.NavType
import androidx.navigation.compose.*
import androidx.navigation.navArgument
import dev.carraes.lam.R
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.requests.*
import dev.carraes.lam.ui.components.QueueTopBar
import dev.carraes.lam.ui.theme.Graphite
import dev.carraes.lam.ui.detail.DecisionDetailScreen
import dev.carraes.lam.ui.detail.DecisionState

@Composable
fun LamNav(state: RequestsState, onQuery: (String) -> Unit, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClearFilters: () -> Unit, onRefresh: () -> Unit,
    onRequestAction: (String, Boolean) -> Unit = { _, _ -> },
    detailContent: @Composable (String, () -> Unit) -> Unit = { _, onBack ->
        DecisionDetailScreen(DecisionState(loading = false, loadFailed = true), onBack)
    }) {
    val nav = rememberNavController()
    fun queue(route: String) { nav.navigate(route) { popUpTo("requests"); launchSingleTop = true } }
    NavHost(navController = nav, startDestination = "requests") {
        composable("requests") {
            RequestsScreen(state, onQuery, onType, onPriority, onClearFilters, onRefresh,
                onHistory = { queue("history") }, onSettings = { nav.navigate("settings") },
                onItem = { nav.navigate("item/${Uri.encode(it)}") },
                onQuickResponse = { onRequestAction(it, false) }, onDismiss = { onRequestAction(it, true) })
        }
        composable("history") {
            Surface(Modifier.fillMaxSize(), color = Graphite) {
                Column(Modifier.safeDrawingPadding()) {
                    QueueTopBar(stringResource(R.string.history_title), { queue("requests") }, {}, { nav.navigate("settings") })
                    Text(stringResource(R.string.history_placeholder), Modifier.padding(24.dp))
                }
            }
        }
        composable("settings") {
            Placeholder(stringResource(R.string.settings_title), stringResource(R.string.settings_placeholder), onBack = { nav.popBackStack() })
        }
        composable("item/{id}", arguments = listOf(navArgument("id") { type = NavType.StringType })) { entry ->
            detailContent(requireNotNull(entry.arguments?.getString("id"))) { nav.popBackStack() }
        }
    }
}

@Composable
private fun Placeholder(title: String, message: String, id: String? = null, onBack: () -> Unit) {
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
            IconButton(onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
            Text(title, style = MaterialTheme.typography.headlineMedium)
            Text(message)
            if (id != null) Text(id, style = MaterialTheme.typography.labelMedium)
        }
    }
}
