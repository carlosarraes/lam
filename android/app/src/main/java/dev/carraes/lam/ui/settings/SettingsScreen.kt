package dev.carraes.lam.ui.settings

import android.content.ClipData
import android.content.Intent
import android.provider.Settings
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.ClipEntry
import androidx.compose.ui.platform.LocalClipboard
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.carraes.lam.BuildConfig
import dev.carraes.lam.R
import dev.carraes.lam.diagnostics.serverOrigin
import dev.carraes.lam.items.SyncState
import dev.carraes.lam.ui.theme.Graphite

@Composable
fun SettingsScreen(viewModel: SettingsViewModel, onBack: () -> Unit) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val clipboard = LocalClipboard.current
    var notifications by remember { mutableStateOf(NotificationManagerCompat.from(context).areNotificationsEnabled()) }
    LifecycleEventEffect(Lifecycle.Event.ON_RESUME) { notifications = NotificationManagerCompat.from(context).areNotificationsEnabled() }
    LaunchedEffect(state.diagnostics) {
        state.diagnostics?.let { clipboard.setClipEntry(ClipEntry(ClipData.newPlainText("lam diagnostics", it))); viewModel.diagnosticsCopied() }
    }
    SettingsScreen(state, BuildConfig.VERSION_NAME, notifications, onBack,
        { context.startActivity(Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)) },
        viewModel::copyDiagnostics, viewModel::requestUnpair, viewModel::confirmUnpair, viewModel::cancelUnpair)
}

@Composable
fun SettingsScreen(state: SettingsState, appVersion: String, notificationsEnabled: Boolean, onBack: () -> Unit,
    onNotifications: () -> Unit, onDiagnostics: () -> Unit, onUnpair: () -> Unit, onConfirm: () -> Unit, onCancel: () -> Unit) {
    Surface(Modifier.fillMaxSize(), color = Graphite) {
        Column(Modifier.safeDrawingPadding().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row {
                IconButton(onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
                Text(stringResource(R.string.settings_title), style = MaterialTheme.typography.headlineMedium)
            }
            SettingsValue(stringResource(R.string.settings_device), state.device?.deviceName.orEmpty())
            SettingsValue(stringResource(R.string.settings_device_id), state.device?.deviceId.orEmpty())
            SettingsValue(stringResource(R.string.settings_server), serverOrigin(state.device?.serverUrl))
            val lastSync = when (val sync = state.sync) {
                is SyncState.Current -> sync.lastSuccess
                is SyncState.Stale -> sync.lastSuccess
                else -> state.lastSuccess
            }
            SettingsValue(stringResource(R.string.settings_last_sync), lastSync?.toString() ?: stringResource(R.string.requests_never_synced))
            SettingsValue(stringResource(R.string.settings_connection), stringResource(when (state.sync) {
                is SyncState.Current -> R.string.settings_connected
                SyncState.Refreshing -> R.string.settings_syncing
                is SyncState.Stale -> R.string.settings_offline
                SyncState.Revoked -> R.string.settings_revoked
                SyncState.Idle -> R.string.settings_waiting
            }))
            Text(stringResource(if (notificationsEnabled) R.string.settings_notifications_enabled else R.string.settings_notifications_disabled))
            TextButton(onNotifications) { Text(stringResource(R.string.settings_notifications_link)) }
            SettingsValue(stringResource(R.string.settings_version), appVersion)
            TextButton(onDiagnostics) { Text(stringResource(R.string.settings_diagnostics)) }
            TextButton(onUnpair, enabled = !state.busy && state.device != null) { Text(stringResource(R.string.settings_unpair)) }
            if (state.failed) Text(stringResource(R.string.settings_failed), color = MaterialTheme.colorScheme.error)
        }
    }
    state.confirmation?.let { confirmation ->
        val local = confirmation == UnpairConfirmation.LOCAL_ERASE
        AlertDialog(onDismissRequest = onCancel,
            title = { Text(stringResource(if (local) R.string.settings_erase_title else R.string.settings_unpair)) },
            text = { Text(stringResource(if (local) R.string.settings_erase_explanation else R.string.settings_unpair_explanation)) },
            confirmButton = { TextButton(onConfirm, enabled = !state.busy) { Text(stringResource(if (local) R.string.settings_erase else R.string.settings_revoke)) } },
            dismissButton = { TextButton(onCancel, enabled = !state.busy) { Text(stringResource(R.string.pairing_cancel)) } })
    }
}

@Composable
private fun SettingsValue(label: String, value: String) {
    Column { Text(label, style = MaterialTheme.typography.labelMedium); Text(value) }
}
