package dev.carraes.lam

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import dev.carraes.lam.ui.theme.LamTheme
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.carraes.lam.pairing.PairingViewModel
import dev.carraes.lam.ui.LamApp

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            LamTheme {
                val container = (application as LamApplication).container
                val pairing = viewModel { PairingViewModel(container.pairingRepository, BuildConfig.DEBUG) }
                LamApp(container, pairing)
            }
        }
    }
}
