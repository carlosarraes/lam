package dev.carraes.lam.pairing

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

enum class PairingProblem {
    MALFORMED, UNSUPPORTED_VERSION, INSECURE_SERVER, EXPIRED, CONSUMED, CANCELLED,
    WRONG_SERVER, OFFLINE, SERVER, STORAGE, CAMERA,
}

class PairingPayloadException(val problem: PairingProblem) : Exception("Invalid pairing code")

/** Never place this object or its raw JSON in saved state, logs, or UI state. */
class PairingPayload private constructor(val serverUrl: String, val session: String, secret: String) {
    var secret: String? = secret
        private set

    fun clearSecret() { secret = null }

    override fun toString() = "PairingPayload(secret=[redacted])"

    companion object {
        private val secretPattern = Regex("^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$")
        private val sessionPattern = Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")

        fun parse(raw: String, allowLocalHttp: Boolean): PairingPayload {
            if (raw.length > 4096) invalid()
            val wire = try { Json.decodeFromString<WirePayload>(raw) } catch (_: Exception) { invalid() }
            if (wire.v != 1) throw PairingPayloadException(PairingProblem.UNSUPPORTED_VERSION)
            if (!sessionPattern.matches(wire.session) || !secretPattern.matches(wire.secret)) invalid()
            val url = wire.server.toHttpUrlOrNull() ?: invalid()
            if (url.username.isNotEmpty() || url.password.isNotEmpty() ||
                url.query != null || url.fragment != null || url.encodedPath != "/" ||
                wire.server.contains('@') || wire.server.contains('\\')) invalid()
            if (!url.isHttps && !(allowLocalHttp && isLocalHost(url.host))) {
                throw PairingPayloadException(PairingProblem.INSECURE_SERVER)
            }
            return PairingPayload(url.toString().removeSuffix("/"), wire.session, wire.secret)
        }

        private fun isLocalHost(host: String): Boolean {
            if (host == "localhost" || host == "::1") return true
            val octets = host.split('.').map { it.toIntOrNull() ?: return false }
            if (octets.size != 4 || octets.any { it !in 0..255 }) return false
            return octets[0] == 127 || octets[0] == 10 ||
                (octets[0] == 192 && octets[1] == 168) ||
                (octets[0] == 172 && octets[1] in 16..31)
        }

        private fun invalid(): Nothing = throw PairingPayloadException(PairingProblem.MALFORMED)
    }
}

@Serializable
private class WirePayload(val v: Int, val server: String, val session: String, val secret: String)
