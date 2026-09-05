package dev.carraes.lam.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.carraes.lam.AppContainer
import dev.carraes.lam.pairing.PairingScreen
import dev.carraes.lam.pairing.PairingViewModel
import dev.carraes.lam.ui.requests.RequestsViewModel
import dev.carraes.lam.ui.detail.DecisionDetailScreen
import dev.carraes.lam.ui.detail.DecisionViewModel

@Composable
fun LamApp(container: AppContainer, pairing: PairingViewModel) {
    PairingScreen(pairing) {
        val requests = viewModel { RequestsViewModel(container.itemRepository, container.lifecycleReconciler::refresh) }
        val state by requests.state.collectAsStateWithLifecycle()
        LamNav(state, requests::setQuery, requests::setType, requests::setPriority, requests::clearFilters, requests::refresh) { id, onBack ->
            val detail = viewModel(key = "decision:$id") { DecisionViewModel(id, container.itemRepository, container.lifecycleReconciler::refresh) }
            DecisionDetailScreen(detail, onBack)
        }
    }
}
