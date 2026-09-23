package dev.carraes.lam.notifications

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.net.toUri
import dev.carraes.lam.MainActivity
import dev.carraes.lam.R

internal class LamNotifications(private val context: Context) {
    private val manager = NotificationManagerCompat.from(context)

    fun createChannels() {
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CRITICAL_CHANNEL, context.getString(R.string.notification_channel_critical), NotificationManager.IMPORTANCE_HIGH).apply {
                description = context.getString(R.string.notification_channel_critical_description)
                lockscreenVisibility = Notification.VISIBILITY_PRIVATE
                enableVibration(true)
            },
        )
    }

    fun handle(event: CriticalPushEvent) {
        when (val plan = PushNotificationPlan.forEvent(event)) {
            is PushNotificationPlan.Cancel -> cancelTag(plan.tag)
            is PushNotificationPlan.Show -> show(plan)
        }
    }

    fun cancel(itemId: String) = cancelTag("$ITEM_TAG_PREFIX$itemId")

    /** Repairs missed close events from the canonical Room state after a full sync. */
    fun reconcile(openCriticalItemIds: Set<String>) {
        context.getSystemService(NotificationManager::class.java).activeNotifications
            .mapNotNull { it.tag }
            .filter { it.startsWith(ITEM_TAG_PREFIX) && it.removePrefix(ITEM_TAG_PREFIX) !in openCriticalItemIds }
            .forEach { manager.cancel(it, NOTIFICATION_ID) }
    }

    private fun show(plan: PushNotificationPlan.Show) {
        createChannels()
        if (ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) return
        val intent = Intent(Intent.ACTION_VIEW, "lam://items/${plan.itemId}".toUri(), context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val pending = PendingIntent.getActivity(
            context,
            plan.itemId.hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val publicVersion = NotificationCompat.Builder(context, CRITICAL_CHANNEL)
            .setSmallIcon(R.drawable.ic_notification_lam)
            .setContentTitle(context.getString(R.string.notification_public_title))
            .setContentText(plan.publicText)
            .build()
        val notification = NotificationCompat.Builder(context, CRITICAL_CHANNEL)
            .setSmallIcon(R.drawable.ic_notification_lam)
            .setContentTitle(plan.title)
            .setContentText(plan.text)
            .setContentIntent(pending)
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
            .build()
        manager.notify(plan.tag, NOTIFICATION_ID, notification)
    }

    private fun cancelTag(tag: String) = manager.cancel(tag, NOTIFICATION_ID)

    companion object {
        const val CRITICAL_CHANNEL = "lam.critical"
        private const val ITEM_TAG_PREFIX = "item:"
        private const val NOTIFICATION_ID = 1
    }
}
