package dev.carraes.lam.items

import java.io.IOException
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerializationException
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.Authenticator
import okhttp3.HttpUrl
import okhttp3.Interceptor
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody

class OkHttpLamApi(
    baseUrl: HttpUrl,
    private val credentialProvider: () -> String?,
    baseClient: OkHttpClient = OkHttpClient(),
    private val json: Json = Json {
        ignoreUnknownKeys = true
        explicitNulls = true
        encodeDefaults = true
    },
) : LamApi {
    private val baseUrl = baseUrl.ensureTrailingSlash()
    private val client = baseClient.newBuilder()
        .connectTimeout(10, TimeUnit.SECONDS)
        .readTimeout(20, TimeUnit.SECONDS)
        .writeTimeout(20, TimeUnit.SECONDS)
        .retryOnConnectionFailure(false)
        .followRedirects(false)
        .followSslRedirects(false)
        .authenticator(Authenticator.NONE)
        .proxyAuthenticator(Authenticator.NONE)
        .addInterceptor(BearerInterceptor(credentialProvider))
        .build()

    override suspend fun listOpenItems(): List<ItemDto> {
        val request = authenticatedRequest("items")
            .url(url("items").newBuilder().addQueryParameter("status", "open").build())
            .get()
            .build()
        return execute(request, ReadPolicy.RETRY_ONCE, ListSerializer(ItemDto.serializer()))
    }

    override suspend fun getItem(id: String): ItemDto = execute(
        authenticatedRequest("items", id).get().build(),
        ReadPolicy.RETRY_ONCE,
        ItemDto.serializer(),
    )

    override suspend fun getHistory(
        query: String?,
        priority: PriorityDto?,
        type: ItemTypeDto?,
        cursor: String?,
        limit: Int,
    ): HistoryPageDto {
        val url = url("history").newBuilder().apply {
            query?.let { addQueryParameter("q", it) }
            priority?.let { addQueryParameter("priority", json.encodeToString(it).trim('"')) }
            type?.let { addQueryParameter("type", json.encodeToString(it).trim('"')) }
            cursor?.let { addQueryParameter("cursor", it) }
            addQueryParameter("limit", limit.toString())
        }.build()
        return execute(
            authenticatedRequest("history").url(url).get().build(),
            ReadPolicy.RETRY_ONCE,
            HistoryPageDto.serializer(),
        )
    }

    override suspend fun replyChoice(id: String, choice: String): ItemDto = postJson(
        segments = arrayOf("items", id, "resolve"),
        body = json.encodeToString(ChoiceResolution(choice)),
        serializer = ItemDto.serializer(),
    )

    override suspend fun complete(id: String): ItemDto = postJson(
        segments = arrayOf("items", id, "resolve"),
        body = "{}",
        serializer = ItemDto.serializer(),
    )

    override suspend fun markSeen(id: String, version: Long): ItemDto = postJson(
        segments = arrayOf("v2", "items", id, "seen"),
        body = buildJsonObject { put("version", version) }.toString(),
        serializer = ItemDto.serializer(),
    )

    override suspend fun replyText(id: String, text: String): ItemDto = postJson(
        segments = arrayOf("items", id, "resolve"),
        body = json.encodeToString(TextResolution(text)),
        serializer = ItemDto.serializer(),
    )

    override suspend fun dismiss(id: String): ItemDto = execute(
        authenticatedRequest("items", id, "dismiss")
            .post(EMPTY_BODY)
            .build(),
        ReadPolicy.NEVER_RETRY,
        ItemDto.serializer(),
    )

    override suspend fun setCheck(id: String, index: Int, done: Boolean): ItemDto = postJson(
        segments = arrayOf("items", id, "checks", index.toString()),
        body = json.encodeToString(CheckUpdate(done)),
        serializer = ItemDto.serializer(),
    )

    override suspend fun getDevice(): DeviceRegistrationDto = execute(
        authenticatedRequest("device").get().build(),
        ReadPolicy.RETRY_ONCE,
        DeviceRegistrationDto.serializer(),
    )

    override suspend fun updateDevice(update: DeviceUpdateDto): DeviceRegistrationDto = execute(
        authenticatedRequest("device")
            .patch(encodeDeviceUpdate(update).toRequestBody(JSON_MEDIA_TYPE))
            .build(),
        ReadPolicy.NEVER_RETRY,
        DeviceRegistrationDto.serializer(),
        sensitiveValues = when (val token = update.fcmToken) {
            is FcmTokenUpdate.Set -> listOf(token.value)
            FcmTokenUpdate.Clear,
            FcmTokenUpdate.Unchanged,
            -> emptyList()
        },
    )

    private fun encodeDeviceUpdate(update: DeviceUpdateDto): String {
        val body = buildJsonObject {
            update.name?.let { put("name", it) }
            when (val fcmToken = update.fcmToken) {
                FcmTokenUpdate.Unchanged -> Unit
                FcmTokenUpdate.Clear -> put("fcm_token", JsonNull)
                is FcmTokenUpdate.Set -> put("fcm_token", fcmToken.value)
            }
            update.appVersion?.let { put("app_version", it) }
            update.androidVersion?.let { put("android_version", it) }
        }
        return json.encodeToString(JsonObject.serializer(), body)
    }

    override suspend fun revokeDevice(): DeviceSummaryDto = execute(
        authenticatedRequest("device")
            .delete()
            .build(),
        ReadPolicy.NEVER_RETRY,
        DeviceSummaryDto.serializer(),
    )

    override suspend fun claimPairing(
        sessionId: String,
        request: PairingClaimRequestDto,
    ): PairingClaimResponseDto = execute(
        Request.Builder()
            .url(url("pairings", sessionId, "claim"))
            .post(json.encodeToString(request).toRequestBody(JSON_MEDIA_TYPE))
            .build(),
        ReadPolicy.NEVER_RETRY,
        PairingClaimResponseDto.serializer(),
        sensitiveValues = listOfNotNull(request.secret, request.fcmToken),
    )

    private suspend fun <T> postJson(
        segments: Array<String>,
        body: String,
        serializer: KSerializer<T>,
    ): T = execute(
        authenticatedRequest(*segments)
            .post(body.toRequestBody(JSON_MEDIA_TYPE))
            .build(),
        ReadPolicy.NEVER_RETRY,
        serializer,
    )

    private fun authenticatedRequest(vararg segments: String): Request.Builder = Request.Builder()
        .url(url(*segments))
        .tag(BearerRequired::class.java, BearerRequired)

    private fun url(vararg segments: String): HttpUrl = baseUrl.newBuilder().apply {
        segments.forEach(::addPathSegment)
    }.build()

    private suspend fun <T> execute(
        request: Request,
        readPolicy: ReadPolicy,
        serializer: KSerializer<T>,
        sensitiveValues: List<String> = emptyList(),
    ): T = withContext(Dispatchers.IO) {
        var attempt = 0
        while (true) {
            try {
                return@withContext client.newCall(request).execute().use { response ->
                    val rawBody = response.body.string()
                    val safeBody = safeResponseBody(
                        rawBody,
                        sensitiveValues + listOfNotNull(response.request.sentBearerCredential()),
                    )
                    if (!response.isSuccessful) throw response.toApiError(rawBody, safeBody)
                    try {
                        json.decodeFromString(serializer, rawBody)
                    } catch (_: SerializationException) {
                        throw ApiError.Server(response.code, safeBody)
                    }
                }
            } catch (error: ApiError) {
                throw error
            } catch (error: IOException) {
                if (readPolicy == ReadPolicy.RETRY_ONCE && attempt == 0) {
                    attempt += 1
                    continue
                }
                throw ApiError.Transport(error.javaClass.simpleName)
            }
        }
        error("unreachable")
    }

    private fun okhttp3.Response.toApiError(rawBody: String, safeBody: String?): ApiError = when (code) {
        400 -> ApiError.Validation(safeBody)
        401 -> ApiError.Unauthorized(safeBody)
        403 -> ApiError.Forbidden(safeBody)
        409 -> when (decodeWorkerError(rawBody)) {
            "already closed" -> ApiError.AlreadyClosed(safeBody)
            "expired" -> ApiError.Server(code, safeBody, ApiConflictCode.PAIRING_EXPIRED)
            "consumed" -> ApiError.Server(code, safeBody, ApiConflictCode.PAIRING_CONSUMED)
            "cancelled" -> ApiError.Server(code, safeBody, ApiConflictCode.PAIRING_CANCELLED)
            "concurrent update, retry" -> ApiError.Server(code, safeBody, ApiConflictCode.CONCURRENT_UPDATE)
            else -> ApiError.Server(code, safeBody)
        }
        else -> ApiError.Server(code, safeBody)
    }

    private fun decodeWorkerError(rawBody: String): String? = runCatching {
        json.decodeFromString(WorkerErrorBody.serializer(), rawBody).error
    }.getOrNull()

    private fun Request.sentBearerCredential(): String? = header("Authorization")
        ?.takeIf { it.startsWith(BEARER_PREFIX) }
        ?.removePrefix(BEARER_PREFIX)

    private enum class ReadPolicy {
        RETRY_ONCE,
        NEVER_RETRY,
    }

    private object BearerRequired

    private class BearerInterceptor(
        private val credentialProvider: () -> String?,
    ) : Interceptor {
        override fun intercept(chain: Interceptor.Chain): okhttp3.Response {
            val request = chain.request()
            if (request.tag(BearerRequired::class.java) == null) return chain.proceed(request)
            val credential = credentialProvider()?.takeIf(String::isNotBlank)
                ?: return chain.proceed(request)
            return chain.proceed(
                request.newBuilder()
                    .header("Authorization", "Bearer $credential")
                    .build(),
            )
        }
    }

    companion object {
        private val JSON_MEDIA_TYPE = "application/json; charset=utf-8".toMediaType()
        private val EMPTY_BODY = ByteArray(0).toRequestBody(null)
        private const val BEARER_PREFIX = "Bearer "
        private val SENSITIVE_JSON_VALUE = Regex(
            "(\"(?:credential|secret|fcm_token|authorization)\"\\s*:\\s*)\"(?:\\\\.|[^\"\\\\])*\"",
            RegexOption.IGNORE_CASE,
        )

        private fun HttpUrl.ensureTrailingSlash(): HttpUrl = if (encodedPath.endsWith('/')) {
            this
        } else {
            newBuilder().addPathSegment("").build()
        }

        private fun safeResponseBody(rawBody: String, sensitiveValues: List<String>): String? {
            if (rawBody.isEmpty()) return null
            val bodyWithoutNamedSecrets = SENSITIVE_JSON_VALUE.replace(rawBody) { match ->
                "${match.groupValues[1]}\"[redacted]\""
            }
            val redacted = sensitiveValues
                .asSequence()
                .filter(String::isNotBlank)
                .distinct()
                .sortedByDescending(String::length)
                .fold(bodyWithoutNamedSecrets) { body, sensitive -> body.replace(sensitive, "[redacted]") }
            return redacted.take(ApiError.MAX_RESPONSE_BODY_CHARACTERS)
        }
    }
}

@kotlinx.serialization.Serializable
private data class ChoiceResolution(val choice: String)

@kotlinx.serialization.Serializable
private data class TextResolution(val text: String)

@kotlinx.serialization.Serializable
private data class CheckUpdate(val done: Boolean)

@kotlinx.serialization.Serializable
private data class WorkerErrorBody(val error: String? = null)
