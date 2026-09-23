package dev.carraes.lam.notifications

import android.annotation.SuppressLint
import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.OutOfQuotaPolicy
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import com.google.firebase.installations.FirebaseInstallations
import dev.carraes.lam.LamApplication
import kotlinx.coroutines.flow.first
import kotlin.coroutines.resume
import kotlin.coroutines.suspendCoroutine

internal suspend fun currentFirebaseInstallationId(): String? = try {
    Log.i("LamPush", "Starting FCM registration")
    suspendCoroutine { continuation ->
        FirebaseMessaging.getInstance().register().addOnCompleteListener { registration ->
            if (!registration.isSuccessful) {
                Log.w("LamPush", "FCM registration failed", registration.exception)
                continuation.resume(null)
            } else {
                Log.i("LamPush", "FCM registration succeeded")
                FirebaseInstallations.getInstance().id.addOnCompleteListener { task ->
                    if (!task.isSuccessful) Log.w("LamPush", "Installation ID lookup failed", task.exception)
                    continuation.resume(task.takeIf { it.isSuccessful }?.result?.takeIf(String::isNotBlank))
                }
            }
        }
    }
} catch (error: Exception) {
    Log.w("LamPush", "FCM registration could not start", error)
    null
}

// Current FID registration uses onRegistered; the lint rule still checks the deprecated token callback.
@SuppressLint("MissingFirebaseInstanceTokenRefresh")
class LamFirebaseMessagingService : FirebaseMessagingService() {
    override fun onMessageReceived(message: RemoteMessage) {
        val event = PushEventParser.parse(message.data) ?: return
        LamNotifications(this).handle(event)
        PushRefreshWorker.enqueue(this, event.itemId)
    }

    override fun onRegistered(installationId: String) {
        Log.i("LamPush", "FCM onRegistered callback received")
        if (installationId.isNotBlank()) (application as LamApplication).container.registerPushToken(installationId)
    }
}

class PushRefreshWorker(context: Context, parameters: WorkerParameters) : CoroutineWorker(context, parameters) {
    override suspend fun doWork(): Result {
        val id = inputData.getString(ITEM_ID)?.takeIf(String::isNotBlank) ?: return Result.failure()
        val repository = (applicationContext as LamApplication).container.itemRepository
        if (!repository.refreshItem(id)) return Result.retry()
        if (!PushNotificationPlan.shouldRemain(repository.item(id).first())) {
            LamNotifications(applicationContext).cancel(id)
        }
        return Result.success()
    }

    companion object {
        private const val ITEM_ID = "item_id"

        fun enqueue(context: Context, id: String) {
            val request = OneTimeWorkRequestBuilder<PushRefreshWorker>()
                .setInputData(workDataOf(ITEM_ID to id))
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setExpedited(OutOfQuotaPolicy.RUN_AS_NON_EXPEDITED_WORK_REQUEST)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork("lam-push-$id", ExistingWorkPolicy.REPLACE, request)
        }
    }
}
