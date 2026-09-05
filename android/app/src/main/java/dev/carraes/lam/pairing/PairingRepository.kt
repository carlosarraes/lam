package dev.carraes.lam.pairing

import dev.carraes.lam.items.ApiConflictCode
import dev.carraes.lam.items.ApiError
import dev.carraes.lam.items.OkHttpLamApi
import dev.carraes.lam.items.PairingClaimRequestDto
import dev.carraes.lam.items.PairingClaimResponseDto
import dev.carraes.lam.security.CredentialStore
import dev.carraes.lam.security.PairedServer
import java.time.Instant
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.withContext
import okhttp3.HttpUrl.Companion.toHttpUrl

data class DeviceIdentity(val name: String, val appVersion: String, val androidVersion: String)
data class PairingResult(val problem: PairingProblem? = null, val stale: Boolean = false)

class PairingRepository(
    private val credentials: CredentialStore,
    private val reconcile: suspend () -> Boolean,
    private val identity: DeviceIdentity,
    private val claim: suspend (String, String, PairingClaimRequestDto) -> PairingClaimResponseDto = { origin, session, request ->
        OkHttpLamApi(origin.toHttpUrl(), credentialProvider = { null }).claimPairing(session, request)
    },
) {
    fun observePairing() = credentials.observe()

    suspend fun pair(payload: PairingPayload): PairingResult {
        val response = try {
            claim(payload.serverUrl, payload.session, PairingClaimRequestDto(
                requireNotNull(payload.secret), identity.name, null, identity.appVersion, identity.androidVersion,
            ))
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: ApiError) {
            return PairingResult(problem = when (error) {
                is ApiError.Transport -> PairingProblem.OFFLINE
                is ApiError.Unauthorized, is ApiError.Forbidden, is ApiError.Validation -> PairingProblem.WRONG_SERVER
                is ApiError.Server -> when (error.conflictCode) {
                    ApiConflictCode.PAIRING_EXPIRED -> PairingProblem.EXPIRED
                    ApiConflictCode.PAIRING_CONSUMED -> PairingProblem.CONSUMED
                    ApiConflictCode.PAIRING_CANCELLED -> PairingProblem.CANCELLED
                    else -> if (error.statusCode == 404 || error.statusCode in 200..399) {
                        PairingProblem.WRONG_SERVER
                    } else PairingProblem.SERVER
                }
                else -> PairingProblem.SERVER
            })
        } catch (_: Exception) {
            return PairingResult(problem = PairingProblem.SERVER)
        } finally {
            payload.clearSecret()
        }
        val device = response.device
        if (response.credential.isBlank() || device.id.isBlank() || device.name != identity.name ||
            device.appVersion != identity.appVersion || device.androidVersion != identity.androidVersion ||
            device.pushRegistered || !validInstant(device.createdAt) ||
            (device.lastSeenAt != null && !validInstant(device.lastSeenAt))) {
            return PairingResult(problem = PairingProblem.WRONG_SERVER)
        }
        currentCoroutineContext().ensureActive()
        try {
            // Complete the local write even if the screen leaves during Keystore persistence.
            withContext(NonCancellable) {
                credentials.save(PairedServer(payload.serverUrl, device.id, device.name), response.credential)
            }
        } catch (_: Exception) {
            return PairingResult(problem = PairingProblem.STORAGE)
        }
        return PairingResult(stale = !refresh())
    }

    suspend fun refresh(): Boolean = try {
        reconcile()
    } catch (_: CancellationException) {
        // A shared refresh can be cancelled on a connection change while this caller remains active.
        currentCoroutineContext().ensureActive()
        false
    } catch (_: Exception) {
        false
    }

    private fun validInstant(value: String) = runCatching { Instant.parse(value) }.isSuccess
}
