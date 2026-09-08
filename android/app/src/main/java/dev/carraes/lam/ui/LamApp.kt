package dev.carraes.lam.ui

import androidx.compose.runtime.*
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.ViewModelStoreOwner
import androidx.lifecycle.ViewModel
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.compose.LifecycleStartEffect
import androidx.lifecycle.viewmodel.compose.viewModel
import androidx.lifecycle.viewmodel.compose.LocalViewModelStoreOwner
import dev.carraes.lam.AppContainer
import dev.carraes.lam.pairing.PairingScreen
import dev.carraes.lam.pairing.PairingViewModel
import dev.carraes.lam.ui.requests.RequestsViewModel
import dev.carraes.lam.ui.history.*
import dev.carraes.lam.ui.settings.*
import dev.carraes.lam.ui.detail.*
import dev.carraes.lam.R
import kotlinx.coroutines.launch
import dev.carraes.lam.articles.*

@Composable
fun LamApp(container: AppContainer, pairing: PairingViewModel) {
    val session by container.deviceSettings.reconciliationSession.collectAsStateWithLifecycle()
    val owner = viewModel { PairedViewModels() }
    owner.bind(session)
    PairingScreen(pairing) {
        key(session) {
            CompositionLocalProvider(LocalViewModelStoreOwner provides owner) { PairedApp(container) }
        }
    }
}

/** The Activity retains this store across rotation; account transitions clear every child. */
internal class PairedViewModels : ViewModel(), ViewModelStoreOwner {
    override val viewModelStore = ViewModelStore()
    private var session: Long? = null
    fun bind(value: Long?) {
        if (session != value) { viewModelStore.clear(); session = value }
    }
    override fun onCleared() { viewModelStore.clear() }
}

@Composable
private fun PairedApp(container: AppContainer) {
        val requests = viewModel { RequestsViewModel(container.itemRepository, container.lifecycleReconciler::refresh) }
        val history = viewModel { HistoryViewModel(container.itemRepository, container.lifecycleReconciler.completedReconciliations) }
        val articles = viewModel { ArticleViewModel(container.articleRepository) }
        val state by requests.state.collectAsStateWithLifecycle()
        var action by remember { mutableStateOf<Pair<String, Boolean>?>(null) }
        val snackbar = remember { SnackbarHostState() }
        val feedbackScope = rememberCoroutineScope()
        LamNav(state, requests::setQuery, requests::setType, requests::setPriority, requests::clearFilters, requests::refresh,
            onRequestAction = { id, dismiss -> action = id to dismiss },
            historyContent = { onRequests, onSettings ->
                LifecycleStartEffect(history) {
                    history.setVisible(true)
                    onStopOrDispose { history.setVisible(false) }
                }
                val historyState by history.state.collectAsStateWithLifecycle()
                HistoryScreen(historyState, history::setQuery, history::setType, history::setPriority,
                    history::clearFilters, history::refresh, history::loadMore, onRequests, onSettings)
            }, articlesContent = { onRequests, onHistory, onSettings, onArticle ->
                val articleState by articles.state.collectAsStateWithLifecycle()
                LaunchedEffect(articles) { articles.refresh() }
                ArticlesScreen(articleState, articles::query, articles::filter, articles::refresh, articles::more,
                    { id -> articles.open(id); onArticle(id) }, articles::unread, onRequests, onHistory, onSettings)
            }, articleContent = { id, onBack ->
                val reader by articles.reader.collectAsStateWithLifecycle()
                LaunchedEffect(id) { if (reader.id != id) articles.open(id) }
                ArticleReader(reader, container.articleRepository, articles::visible, { articles.close(); onBack() }, articles::reload)
            }, settingsContent = { onBack ->
                val settings = viewModel { SettingsViewModel(container.deviceSettings, container.diagnostics) }
                SettingsScreen(settings, onBack)
            }) { id, onBack ->
            val detail = viewModel(key = "decision:$id") { DecisionViewModel(id, container.itemRepository, container.lifecycleReconciler::refresh) }
            val checklist = viewModel(key = "checks:$id") { ChecklistViewModel(id, container.itemRepository) }
            DecisionDetailScreen(detail, onBack, checklist)
        }
        val articleFeedback by articles.feedback.collectAsStateWithLifecycle()
        val articleMessage = articleFeedback?.let { stringResource(readMessage(it)) }
        LaunchedEffect(articleMessage) {
            articleMessage?.let { snackbar.showSnackbar(it); articles.consumeFeedback() }
        }
        action?.let { (id, dismiss) -> key(id, dismiss) {
            RequestActionHost(container, id, dismiss, snackbar, { message -> feedbackScope.launch { snackbar.showSnackbar(message) } }) { action = null }
        } }
        Box(Modifier.fillMaxSize()) { SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).safeDrawingPadding()) }
}

@Composable
private fun RequestActionHost(container: AppContainer, id: String, dismiss: Boolean, snackbar: SnackbarHostState,
    onFeedback: (String) -> Unit, onClose: () -> Unit) {
    val owner = remember { object : ViewModelStoreOwner { override val viewModelStore = ViewModelStore() } }
    DisposableEffect(owner) { onDispose { owner.viewModelStore.clear() } }
    val decision = viewModel(viewModelStoreOwner = owner) { DecisionViewModel(id, container.itemRepository, container.lifecycleReconciler::refresh) }
    val checklist = viewModel(viewModelStoreOwner = owner) { ChecklistViewModel(id, container.itemRepository) }
    val state by decision.state.collectAsStateWithLifecycle()
    val checks by checklist.state.collectAsStateWithLifecycle()
    var started by remember { mutableStateOf(false) }
    val answerFailed = stringResource(R.string.detail_answer_failed)
    val checkFailed = stringResource(R.string.check_failed)
    LaunchedEffect(state.loading, state.refreshing, state.actionsEnabled) {
        if (!started && !state.loading && !state.refreshing) {
            started = true
            if (dismiss) decision.dismissRequest() else decision.quickResponse()
        }
    }
    LaunchedEffect(started, state.quickOpen, state.replyOpen, state.confirmation, state.submitting, checks.saving) {
        if (started && !state.quickOpen && !state.replyOpen && state.confirmation == null && !state.submitting && !checks.saving) {
            if (state.answerFailed) onFeedback(answerFailed)
            onClose()
        }
    }
    LaunchedEffect(checks.failure) {
        checks.failure?.let { checklist.consumeFailure(it); onFeedback(checkFailed) }
    }
    DecisionActionSheets(state, checks, decision::choose, checklist::setCheck, decision::writeReply,
        decision::editReply, decision::reviewReply, decision::dismissReply, decision::confirm,
        decision::dismissConfirmation, decision::closeQuickResponse, snackbar)
}
