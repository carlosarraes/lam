package dev.carraes.lam.ui

import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import dev.carraes.lam.ui.components.LocalArticlesNavigation
import dev.carraes.lam.articles.ArticlesScreen
import dev.carraes.lam.articles.ArticlesState
import androidx.navigation.NavType
import androidx.navigation.compose.*
import androidx.navigation.navArgument
import dev.carraes.lam.items.*
import dev.carraes.lam.ui.requests.*
import dev.carraes.lam.ui.history.*
import dev.carraes.lam.ui.settings.*
import dev.carraes.lam.ui.detail.DecisionDetailScreen
import dev.carraes.lam.ui.detail.DecisionState

@Composable
fun LamNav(state: RequestsState, onQuery: (String) -> Unit, onType: (ItemTypeDto?) -> Unit,
    onPriority: (PriorityDto?) -> Unit, onClearFilters: () -> Unit, onRefresh: () -> Unit,
    onRequestAction: (String, Boolean) -> Unit = { _, _ -> },
    historyContent: @Composable (() -> Unit, () -> Unit) -> Unit = { onRequests, onSettings ->
        HistoryScreen(HistoryState(), {}, {}, {}, {}, {}, {}, onRequests, onSettings)
    },
    settingsContent: @Composable (() -> Unit) -> Unit = { onBack ->
        SettingsScreen(SettingsState(), dev.carraes.lam.BuildConfig.VERSION_NAME, false, onBack, {}, {}, {}, {}, {})
    },
    articlesContent: @Composable (() -> Unit, () -> Unit, () -> Unit, (String) -> Unit) -> Unit = { onRequests, onHistory, onSettings, onArticle ->
        ArticlesScreen(ArticlesState(), {}, {}, {}, {}, onArticle, {}, onRequests, onHistory, onSettings)
    },
    articleContent: @Composable (String, () -> Unit) -> Unit = { _, onBack -> androidx.compose.material3.TextButton(onBack) { androidx.compose.material3.Text("Back") } },
    detailContent: @Composable (String, () -> Unit) -> Unit = { _, onBack ->
        DecisionDetailScreen(DecisionState(loading = false, loadFailed = true), onBack)
    }) {
    val nav = rememberNavController()
    fun queue(route: String) { nav.navigate(route) { popUpTo("requests") { saveState = true }; launchSingleTop = true; restoreState = true } }
    CompositionLocalProvider(LocalArticlesNavigation provides { queue("articles") }) {
    NavHost(navController = nav, startDestination = "requests") {
        composable("requests") {
            RequestsScreen(state, onQuery, onType, onPriority, onClearFilters, onRefresh,
                onHistory = { queue("history") }, onSettings = { nav.navigate("settings") },
                onItem = { nav.navigate("item/${Uri.encode(it)}") },
                onQuickResponse = { onRequestAction(it, false) }, onDismiss = { onRequestAction(it, true) })
        }
        composable("history") {
            historyContent({ queue("requests") }, { nav.navigate("settings") })
        }
        composable("settings") {
            settingsContent { nav.popBackStack() }
        }
        composable("articles") {
            articlesContent({ queue("requests") }, { queue("history") }, { nav.navigate("settings") }, { nav.navigate("article/${Uri.encode(it)}") })
        }
        composable("article/{id}", arguments = listOf(navArgument("id") { type = NavType.StringType })) { entry ->
            articleContent(requireNotNull(entry.arguments?.getString("id"))) { nav.popBackStack() }
        }
        composable("item/{id}", arguments = listOf(navArgument("id") { type = NavType.StringType })) { entry ->
            detailContent(requireNotNull(entry.arguments?.getString("id"))) { nav.popBackStack() }
        }
    }
    }
}
