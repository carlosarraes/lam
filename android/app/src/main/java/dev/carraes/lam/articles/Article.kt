package dev.carraes.lam.articles

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import java.security.MessageDigest

internal val articleJson = Json { ignoreUnknownKeys = true; encodeDefaults = true }

@Serializable
data class ArticleAsset(val path: String, @SerialName("media_type") val mediaType: String,
    val size: Long, val sha256: String, val disposition: String) {
    val inlineImage: Boolean get() = disposition == "inline" && mediaType in setOf("image/png", "image/jpeg", "image/webp")
}

@Serializable
data class Article(val id: String, val title: String, val summary: String, val name: String,
    @SerialName("source_host") val sourceHost: String, @SerialName("source_project") val sourceProject: String,
    @SerialName("created_at") val createdAt: String, @SerialName("read_at") val readAt: String?,
    val version: Long, val assets: List<ArticleAsset>)

@Serializable
data class ArticlePage(val items: List<Article>, @SerialName("next_cursor") val nextCursor: String?)

@Serializable
data class ArticleContent(val article: Article, val parts: List<JsonElement>)

data class ArticleSession(val account: String, val epoch: String, val generation: Long = 0)
data class LoadedArticle(val article: Article, val html: String, val session: ArticleSession, val cached: Boolean = false)
enum class ReadResult { SAVED, CONFLICT, FAILED }

internal fun sha256(bytes: ByteArray): String = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
