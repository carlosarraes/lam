package dev.carraes.lam.diagnostics

import java.time.Instant
import dev.carraes.lam.items.ApiError
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

data class ItemCounts(val total: Int, val open: Int) { val closed: Int get() = total - open }
enum class ErrorCategory { TRANSPORT, UNAUTHORIZED, FORBIDDEN, CLOSED, VALIDATION, SERVER, LOCAL }
data class DiagnosticData(val serverUrl: String?, val lastSuccess: Instant?, val counts: ItemCounts, val recentErrors: List<ErrorCategory>)

class Diagnostics(private val appVersion: String, private val androidVersion: String, private val deviceVersion: String) {
    fun render(data: DiagnosticData): String = buildString {
        appendLine("App: $appVersion")
        appendLine("Android: $androidVersion")
        appendLine("Device: $deviceVersion")
        appendLine("Server: ${serverOrigin(data.serverUrl)}")
        appendLine("Last sync: ${data.lastSuccess ?: "Never"}")
        appendLine("Open items: ${data.counts.open}")
        appendLine("Closed items: ${data.counts.closed}")
        appendLine("Total items: ${data.counts.total}")
        append("Recent errors: ${data.recentErrors.joinToString().ifEmpty { "None" }}")
    }
}

fun serverOrigin(url: String?): String = url?.toHttpUrlOrNull()?.newBuilder()
    ?.username("")?.password("")?.encodedPath("/")?.query(null)?.fragment(null)?.build()?.toString()?.removeSuffix("/") ?: "Unpaired"

internal fun errorCategory(error: Exception): ErrorCategory = when (error) {
    is ApiError.Transport -> ErrorCategory.TRANSPORT
    is ApiError.Unauthorized -> ErrorCategory.UNAUTHORIZED
    is ApiError.Forbidden -> ErrorCategory.FORBIDDEN
    is ApiError.AlreadyClosed -> ErrorCategory.CLOSED
    is ApiError.Validation -> ErrorCategory.VALIDATION
    is ApiError.Server -> ErrorCategory.SERVER
    else -> ErrorCategory.LOCAL
}
