package dev.carraes.lam.sync

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest

/** Passive application-lifetime callbacks; no network requests or traffic binding. */
internal class AndroidConnectivityMonitor(context: Context) {
    private val manager = context.getSystemService(ConnectivityManager::class.java)
    private val tracker = ConnectivityTracker(null, false)
    val state = tracker.state

    init {
        manager.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = tracker.available(network)
            override fun onLost(network: Network) = tracker.lost(network)
            override fun onCapabilitiesChanged(network: Network, networkCapabilities: NetworkCapabilities) =
                tracker.capabilities(network, networkCapabilities.isValidated(), networkCapabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN))
        })
        // A VPN may retain VALIDATED after its last physical network disappears, and need not
        // propagate underlying transports. Observe non-VPN INTERNET networks independently.
        manager.registerNetworkCallback(NetworkRequest.Builder()
            .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .addCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            .build(), object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) = tracker.physicalAvailable(network)
            override fun onLost(network: Network) = tracker.physicalLost(network)
        })
        // Snapshot only at startup, after registration. Delivered callbacks win over the seed.
        // allNetworks is available on API 29; callbacks replace it for steady-state observation.
        @Suppress("DEPRECATION")
        val physicalNetworks = manager.allNetworks.filter { network ->
            manager.getNetworkCapabilities(network)?.let {
                it.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) &&
                    it.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
            } == true
        }.toSet()
        tracker.seedPhysicalNetworks(physicalNetworks)
        val initialNetwork = manager.activeNetwork
        val capabilities = manager.getNetworkCapabilities(initialNetwork)
        tracker.seedDefault(initialNetwork, capabilities.isValidated(), capabilities?.hasTransport(NetworkCapabilities.TRANSPORT_VPN) == true)
    }
}

private fun NetworkCapabilities?.isValidated(): Boolean = this != null &&
    hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) && hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)
