package dev.carraes.lam.items

sealed class ApiError(
    message: String,
    val statusCode: Int?,
    val responseBody: String?,
) : Exception(message) {
    class Unauthorized internal constructor(responseBody: String?) :
        ApiError("Worker rejected the device credential", 401, responseBody)

    class Forbidden internal constructor(responseBody: String?) :
        ApiError("Worker denied this operation", 403, responseBody)

    class AlreadyClosed internal constructor(responseBody: String?) :
        ApiError("The request is no longer open", 409, responseBody)

    class Validation internal constructor(responseBody: String?) :
        ApiError("Worker rejected the request", 400, responseBody)

    class Transport internal constructor(val failureType: String) :
        ApiError("Could not reach the Worker", null, null)

    class Server internal constructor(
        statusCode: Int,
        responseBody: String?,
        val conflictCode: ApiConflictCode? = null,
    ) :
        ApiError("Worker returned an unexpected response", statusCode, responseBody)

    companion object {
        const val MAX_RESPONSE_BODY_CHARACTERS = 4_096
    }
}

enum class ApiConflictCode {
    PAIRING_EXPIRED,
    PAIRING_CONSUMED,
    PAIRING_CANCELLED,
    CONCURRENT_UPDATE,
}
