package dev.carraes.lam.articles

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import okhttp3.Authenticator
import okhttp3.HttpUrl
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.util.concurrent.TimeUnit

interface ArticleApi {
    fun events(): kotlinx.coroutines.flow.Flow<Unit> = kotlinx.coroutines.flow.emptyFlow()
    suspend fun list(query: String, read: String, cursor: String?, day: String? = null): ArticlePage
    suspend fun get(id: String): Article
    suspend fun content(id: String): ArticleContent
    suspend fun setRead(id: String, read: Boolean, version: Long): Article
    suspend fun asset(id: String, index: Int, limit: Long): ByteArray
}

class ArticleHttpException(val status: Int) : IOException("Article request failed ($status)")

class OkHttpArticleApi(private val baseUrl: HttpUrl, private val credential: () -> String?, baseClient: OkHttpClient = OkHttpClient()) : ArticleApi {
    private val client = baseClient.newBuilder().followRedirects(false).followSslRedirects(false)
        .retryOnConnectionFailure(false).authenticator(Authenticator.NONE).proxyAuthenticator(Authenticator.NONE)
        .connectTimeout(10, TimeUnit.SECONDS).readTimeout(20, TimeUnit.SECONDS).callTimeout(30, TimeUnit.SECONDS).build()
    private fun url(vararg path: String) = baseUrl.newBuilder().apply { addPathSegments("v2/articles"); path.forEach(::addPathSegment) }.build()
    override fun events() = articleEvents(client, baseUrl, credential)
    override suspend fun list(query: String, read: String, cursor: String?, day: String?): ArticlePage {
        val url = url().newBuilder().addQueryParameter("q", query).addQueryParameter("read", read).apply {
            cursor?.let { addQueryParameter("cursor", it) }
            day?.let { addQueryParameter("day", it) }
        }.build()
        return articleJson.decodeFromString(execute(Request.Builder().url(url), 4L * 1024 * 1024).decodeToString(throwOnInvalidSequence = true))
    }
    override suspend fun get(id: String): Article = articleJson.decodeFromString(execute(Request.Builder().url(url(id)), 256 * 1024).decodeToString(throwOnInvalidSequence = true))
    override suspend fun content(id: String): ArticleContent = articleJson.decodeFromString(execute(Request.Builder().url(url(id, "content")), 16L * 1024 * 1024).decodeToString(throwOnInvalidSequence = true))
    override suspend fun setRead(id: String, read: Boolean, version: Long): Article {
        val body = buildJsonObject { put("read", read); put("version", version) }.toString()
        return articleJson.decodeFromString(execute(Request.Builder().url(url(id, "read")).put(body.toRequestBody("application/json".toMediaType())), 256 * 1024).decodeToString(throwOnInvalidSequence = true))
    }
    override suspend fun asset(id: String, index: Int, limit: Long) = execute(Request.Builder().url(url(id, "assets", index.toString())), limit)

    private suspend fun execute(builder: Request.Builder, limit: Long): ByteArray = withContext(Dispatchers.IO) {
        val token = credential() ?: throw ArticleHttpException(401)
        client.newCall(builder.header("Authorization", "Bearer $token").header("Cache-Control", "no-store").build()).execute().use { response ->
            if (!response.isSuccessful) throw ArticleHttpException(response.code)
            val body = response.body
            if (body.contentLength() > limit) throw IOException("Article response too large")
            body.byteStream().use { input ->
                val output = ByteArrayOutputStream()
                val buffer = ByteArray(8192)
                var total = 0L
                while (true) {
                    val count = input.read(buffer)
                    if (count < 0) break
                    total += count
                    if (total > limit) throw IOException("Article response too large")
                    output.write(buffer, 0, count)
                }
                if (credential() != token) throw ArticleHttpException(401)
                output.toByteArray()
            }
        }
    }
}
