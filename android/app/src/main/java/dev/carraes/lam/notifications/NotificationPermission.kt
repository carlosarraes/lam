package dev.carraes.lam.notifications

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.core.content.ContextCompat
import androidx.core.content.edit
import dev.carraes.lam.R

@Composable
internal fun CriticalNotificationPermission(paired: Boolean) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
    val context = LocalContext.current
    val preferences = remember { context.getSharedPreferences("lam-notifications", Context.MODE_PRIVATE) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        preferences.edit { putBoolean("asked", true) }
    }
    var explaining by remember { mutableStateOf(false) }
    LaunchedEffect(paired) {
        if (paired && !preferences.getBoolean("asked", false) &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) explaining = true
    }
    if (explaining) AlertDialog(
        onDismissRequest = { explaining = false },
        title = { Text(stringResource(R.string.notification_permission_title)) },
        text = { Text(stringResource(R.string.notification_permission_explanation)) },
        confirmButton = {
            TextButton(onClick = {
                explaining = false
                launcher.launch(Manifest.permission.POST_NOTIFICATIONS)
            }) { Text(stringResource(R.string.notification_permission_allow)) }
        },
        dismissButton = {
            TextButton(onClick = {
                preferences.edit { putBoolean("asked", true) }
                explaining = false
            }) { Text(stringResource(R.string.notification_permission_not_now)) }
        },
    )
}
