package dev.carraes.lam.sync

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow

data class ConnectivityStatus(val available: Boolean, val epoch: Long)

internal class ConnectivityTracker(initialNetwork: Any?, initialAvailable: Boolean, initialVpn: Boolean = false) {
    private var currentNetwork = initialNetwork
    private var defaultValidated = initialAvailable
    private var defaultVpn = initialVpn
    private var defaultCallbackReceived = false
    private val physicalNetworks = mutableSetOf<Any>()
    private var physicalCallbacksBeforeSeed: MutableSet<Any>? = mutableSetOf()
    private val status = MutableStateFlow(ConnectivityStatus(initialAvailable && !initialVpn, 0))
    val state = status.asStateFlow()

    @Synchronized fun available(network: Any) {
        defaultCallbackReceived = true
        if (network != currentNetwork) {
            currentNetwork = network
            defaultValidated = false
            publish()
        }
    }

    @Synchronized fun capabilities(network: Any, validated: Boolean, vpn: Boolean = false) {
        defaultCallbackReceived = true
        if (network == currentNetwork) {
            defaultValidated = validated
            defaultVpn = vpn
            publish()
        }
    }

    @Synchronized fun physicalAvailable(network: Any) {
        physicalCallbacksBeforeSeed?.add(network)
        physicalNetworks.add(network)
        publish()
    }

    @Synchronized fun physicalLost(network: Any) {
        physicalCallbacksBeforeSeed?.add(network)
        physicalNetworks.remove(network)
        publish()
    }

    @Synchronized fun seedPhysicalNetworks(networks: Set<Any>) {
        val callbacks = physicalCallbacksBeforeSeed ?: return
        physicalNetworks.addAll(networks - callbacks)
        physicalCallbacksBeforeSeed = null
        publish()
    }

    @Synchronized fun seedDefault(network: Any?, validated: Boolean, vpn: Boolean) {
        if (defaultCallbackReceived) return
        currentNetwork = network
        defaultValidated = validated
        defaultVpn = vpn
        publish()
    }

    @Synchronized fun lost(network: Any) {
        defaultCallbackReceived = true
        if (network == currentNetwork) {
            currentNetwork = null
            defaultValidated = false
            publish()
        }
    }

    private fun publish() {
        val available = currentNetwork != null && defaultValidated && (!defaultVpn || physicalNetworks.isNotEmpty())
        val previous = status.value
        if (available != previous.available) status.value = ConnectivityStatus(available, previous.epoch + 1)
    }
}
