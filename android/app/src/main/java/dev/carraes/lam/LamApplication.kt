package dev.carraes.lam

import android.app.Application
import dev.carraes.lam.notifications.LamNotifications

class LamApplication : Application() {
    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        LamNotifications(this).createChannels()
        container = AppContainer(applicationContext)
    }
}
