package dev.carraes.lam.articles

import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.buffer
import kotlinx.coroutines.flow.callbackFlow
import kotlinx.serialization.json.*
import okhttp3.*
import java.io.IOException
import java.util.concurrent.TimeUnit

internal fun isArticleInvalidation(text: String): Boolean = runCatching {
    if (text.length > 4096) return false
    val event = articleJson.parseToJsonElement(text).jsonObject
    event.keys == setOf("event", "article_id", "version") &&
        event["event"]?.jsonPrimitive?.content in setOf("article.published", "article.read_changed") &&
        event["article_id"]?.jsonPrimitive?.content?.matches(Regex("[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}")) == true &&
        event["version"]?.jsonPrimitive?.let { !it.isString && (it.longOrNull ?: -1) >= 0 } == true
}.getOrDefault(false)

/** Native headers only, on the same immutable credential snapshot as the Article API. */
internal fun articleEvents(client: OkHttpClient, baseUrl: HttpUrl, credential: () -> String?) = callbackFlow {
    val token = credential() ?: throw ArticleHttpException(401)
    val request = Request.Builder().url(baseUrl.newBuilder().addPathSegments("v2/events").build())
        .header("Authorization", "Bearer $token").build()
    val socket = client.newBuilder().pingInterval(30, TimeUnit.SECONDS).build().newWebSocket(request, object : WebSocketListener() {
        private fun current(socket: WebSocket): Boolean {
            if (credential() == token) return true
            socket.cancel(); close(ArticleHttpException(401)); return false
        }
        override fun onOpen(webSocket: WebSocket, response: Response) {
            if (current(webSocket)) trySend(Unit)
        }
        override fun onMessage(webSocket: WebSocket, text: String) {
            if (current(webSocket) && isArticleInvalidation(text)) trySend(Unit)
        }
        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) { webSocket.close(code, null); close() }
        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) { close() }
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            close(if (response?.code == 401) ArticleHttpException(401) else IOException("Article event stream unavailable"))
        }
    })
    awaitClose { socket.cancel() }
}.buffer(Channel.CONFLATED)
