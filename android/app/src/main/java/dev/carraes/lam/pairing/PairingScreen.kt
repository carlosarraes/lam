package dev.carraes.lam.pairing

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.carraes.lam.R
import dev.carraes.lam.RequestsSmokeSurface
import dev.carraes.lam.ui.theme.Graphite

@Composable
fun PairingScreen(viewModel: PairingViewModel) {
    val context = LocalContext.current
    val state by viewModel.state.collectAsStateWithLifecycle()
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission(), viewModel::permissionResult)
    BackHandler(state == PairingState.Scanning, viewModel::cancelScan)
    PairingContent(
        state = state,
        onScan = {
            if (ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
                viewModel.permissionResult(true)
            } else permission.launch(Manifest.permission.CAMERA)
        },
        onSettings = { context.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, "package:${context.packageName}".toUri())) },
        onCancel = viewModel::cancelScan,
        onRetryRefresh = viewModel::retryRefresh,
        scanner = { QrScanner(viewModel::onQr, viewModel::cameraFailed) },
    )
}

@Composable
internal fun PairingContent(
    state: PairingState,
    onScan: () -> Unit,
    onSettings: () -> Unit,
    onCancel: () -> Unit,
    onRetryRefresh: () -> Unit,
    scanner: @Composable () -> Unit,
) {
    if (state is PairingState.Paired) {
        RequestsSmokeSurface(state.stale, onRetryRefresh)
        return
    }
    Surface(Modifier.fillMaxSize().testTag("pairing-surface"), color = Graphite) {
        Column(Modifier.safeDrawingPadding().padding(24.dp).verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(20.dp)) {
            Text(stringResource(R.string.app_name), style = MaterialTheme.typography.displayLarge)
            when (state) {
                PairingState.Loading -> CircularProgressIndicator()
                PairingState.Claiming -> {
                    Text(stringResource(R.string.pairing_claiming))
                    CircularProgressIndicator()
                }
                PairingState.Scanning -> {
                    Text(stringResource(R.string.pairing_aim))
                    Box(Modifier.fillMaxWidth().height(320.dp)) { scanner() }
                    TextButton(onCancel) { Text(stringResource(R.string.pairing_cancel)) }
                }
                else -> {
                    Text(stringResource(R.string.pairing_title), style = MaterialTheme.typography.headlineMedium)
                    Text(stringResource(R.string.pairing_instructions))
                    if (state == PairingState.PermissionDenied) Text(stringResource(R.string.pairing_permission_denied))
                    if (state is PairingState.Failed) Text(stringResource(state.problem.messageResource()))
                    Button(onScan) { Text(stringResource(R.string.pairing_scan)) }
                    if (state == PairingState.PermissionDenied) {
                        TextButton(onSettings) { Text(stringResource(R.string.pairing_settings)) }
                    }
                }
            }
        }
    }
}

private fun PairingProblem.messageResource(): Int = when (this) {
    PairingProblem.MALFORMED -> R.string.pairing_malformed
    PairingProblem.UNSUPPORTED_VERSION -> R.string.pairing_unsupported
    PairingProblem.INSECURE_SERVER -> R.string.pairing_insecure
    PairingProblem.EXPIRED -> R.string.pairing_expired
    PairingProblem.CONSUMED -> R.string.pairing_consumed
    PairingProblem.CANCELLED -> R.string.pairing_cancelled
    PairingProblem.WRONG_SERVER -> R.string.pairing_wrong_server
    PairingProblem.OFFLINE -> R.string.pairing_offline
    PairingProblem.SERVER -> R.string.pairing_server_error
    PairingProblem.STORAGE -> R.string.pairing_storage_error
    PairingProblem.CAMERA -> R.string.pairing_camera_error
}
