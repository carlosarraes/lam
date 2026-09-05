package dev.carraes.lam.pairing

import androidx.activity.ComponentActivity
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.collectAsState
import androidx.compose.material3.Button
import androidx.compose.material3.Text
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import androidx.compose.ui.test.junit4.v2.createAndroidComposeRule
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.performClick
import androidx.test.ext.junit.runners.AndroidJUnit4
import dev.carraes.lam.ui.theme.LamTheme
import dev.carraes.lam.items.DeviceRegistrationDto
import dev.carraes.lam.items.PairingClaimResponseDto
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PairingScreenTest {
    @Test fun revokedPairingExplainsWhyAndOffersScanning() {
        compose.setContent { LamTheme { PairingContent(PairingState.Revoked, {}, {}, {}, {}, {}) } }
        compose.onNodeWithText("This device was revoked").assertExists()
        compose.onNodeWithText("Scan pairing code").assertExists()
    }
    @get:Rule val compose = createAndroidComposeRule<ComponentActivity>()

    @Before fun keepTestActivityAwake() {
        compose.runOnUiThread { compose.activity.window.addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) }
    }

    @Test fun cameraPermissionIsRequestedOnlyOnScanAndDenialOffersRetryAndSettings() {
        var requests = 0; var settings = 0
        var state by mutableStateOf<PairingState>(PairingState.Ready)
        compose.setContent { LamTheme { PairingContent(state, onScan = { requests++; state = PairingState.PermissionDenied },
            onSettings = { settings++ }, onCancel = {}, pairedContent = {}, scanner = {}) } }
        compose.runOnIdle { assertEquals(0, requests) }
        compose.onNodeWithText("Scan pairing code").performClick()
        compose.onNodeWithText("Camera access is needed to scan your terminal QR code.").assertExists()
        compose.onNodeWithText("Open settings").performClick()
        compose.onNodeWithText("Scan pairing code").performClick()
        compose.runOnIdle { assertEquals(2, requests); assertEquals(1, settings) }
    }

    @Test fun successfulPairingHandsOffToThePairedApplication() {
        var state by mutableStateOf<PairingState>(PairingState.Claiming)
        compose.setContent { LamTheme { PairingContent(state, {}, {}, {}, { Text("Paired application") }, {}) } }
        compose.onNodeWithText("Pairing device…").assertExists()
        compose.onNodeWithText("Scan pairing code").assertDoesNotExist()
        compose.runOnIdle { state = PairingState.Paired(true) }
        compose.onNodeWithText("Paired application").assertExists()
        compose.onNodeWithText("Scan pairing code").assertDoesNotExist()
    }

    @Test fun scanningCanBeCancelledAndLoadingNeverOffersScan() {
        var state by mutableStateOf<PairingState>(PairingState.Loading)
        compose.setContent { LamTheme { PairingContent(state, {}, {}, { state = PairingState.Ready }, {}, {}) } }
        compose.onNodeWithText("Scan pairing code").assertDoesNotExist()
        compose.runOnIdle { state = PairingState.Scanning }
        compose.onNodeWithText("Cancel").performClick()
        compose.onNodeWithText("Scan pairing code").assertExists()
    }

    @Test fun decodedQrPersistsDeviceAndNavigatesThroughRealViewModel() {
        val metadata = MutableStateFlow<PairedServer?>(null)
        var saved = false
        val store = object : CredentialStore {
            override fun observe() = metadata
            override suspend fun save(server: PairedServer, credential: String) {
                saved = credential == "test-permanent-credential"
                metadata.value = server
            }
            override suspend fun clear() { metadata.value = null }
        }
        val repo = PairingRepository(store, { saved }, DeviceIdentity("Phone", "0.1.0", "16")) { _, _, _ ->
            PairingClaimResponseDto("test-permanent-credential", DeviceRegistrationDto(
                "device-test", "Phone", "0.1.0", "16", "2026-09-04T00:00:00Z", null, false,
            ))
        }
        compose.setContent {
            val vm = ViewModelProvider(compose.activity, viewModelFactory {
                initializer { PairingViewModel(repo, false) }
            })[PairingViewModel::class.java]
            val state by vm.state.collectAsState()
            LamTheme {
                PairingContent(state, { vm.permissionResult(true) }, {}, vm::cancelScan, { Text("Paired application") }) {
                    Button(onClick = { vm.onQr("""{"v":1,"server":"https://lam.example","session":"550e8400-e29b-41d4-a716-446655440000","secret":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}""") }) {
                        Text("Deliver decoded QR")
                    }
                }
            }
        }
        compose.onNodeWithText("Scan pairing code").performClick()
        compose.onNodeWithText("Deliver decoded QR").performClick()
        compose.onNodeWithText("Paired application").assertExists()
        compose.runOnIdle {
            assertEquals(true, saved)
            assertEquals("https://lam.example", metadata.value?.serverUrl)
        }
    }
}
