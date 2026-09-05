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

@Composable
fun LamNav(state: RequestsState, onQuery: (String) -> Unit, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClearFilters: () -> Unit, onRefresh: () -> Unit) {
    val nav = rememberNavController()
    fun queue(route: String) { nav.navigate(route) { popUpTo("requests"); launchSingleTop = true } }
    NavHost(navController = nav, startDestination = "requests") {
        composable("requests") {
            RequestsScreen(state, onQuery, onType, onPriority, onClearFilters, onRefresh,
                onHistory = { queue("history") }, onSettings = { nav.navigate("settings") },
                onItem = { nav.navigate("item/${Uri.encode(it)}") })
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
            Placeholder(stringResource(R.string.request_detail), stringResource(R.string.request_detail_placeholder),
                id = entry.arguments?.getString("id"), onBack = { nav.popBackStack() })
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
