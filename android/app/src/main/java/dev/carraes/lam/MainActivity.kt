package dev.carraes.lam

import android.os.Bundle
import android.content.Intent
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import dev.carraes.lam.ui.theme.LamTheme
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.carraes.lam.pairing.PairingViewModel
import dev.carraes.lam.ui.LamApp
import dev.carraes.lam.notifications.ItemLinks

class MainActivity : ComponentActivity() {
    private val itemLinks by viewModels<ItemLinks>()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) itemLinks.receive(intent.action, intent.dataString)
        else savedInstanceState.getString("pendingItem")?.let { itemLinks.receive(Intent.ACTION_VIEW, "lam://items/$it") }
        setContent {
            LamTheme {
                val container = (application as LamApplication).container
                val pairing = viewModel { PairingViewModel(container.pairingRepository, BuildConfig.DEBUG, container.itemRepository.syncState) }
                val itemRoute by itemLinks.pending.collectAsStateWithLifecycle()
                LamApp(container, pairing, itemRoute, itemLinks::consume)
            }
        }
    }
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        itemLinks.receive(intent.action, intent.dataString)
    }
    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("pendingItem", itemLinks.pending.value)
        super.onSaveInstanceState(outState)
    }
}
