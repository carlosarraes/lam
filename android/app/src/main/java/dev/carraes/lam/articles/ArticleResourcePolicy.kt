package dev.carraes.lam.articles

import java.net.URI
import kotlinx.serialization.json.*

/** This origin is never contacted. Each reader allows only its exact manifest URL map. */
class ArticleResourcePolicy(epoch: String, private val article: Article) {
    init {
        require(epoch.matches(Regex("[A-Za-z0-9-]{1,100}")))
        require(article.id.matches(Regex("[a-f0-9-]{36}")))
        require(article.assets.size in 1..51)
    }
    private val base = "https://articles.lam.invalid/$epoch/${article.id}"
    val documentUrl = "$base/index.html"
    fun assetUrl(index: Int) = "$base/assets/$index"
    private val automatic = article.assets.indices.filter { article.assets[it].inlineImage }.associateBy(::assetUrl)
    fun allowsAutomaticRequest(url: String) = automatic.containsKey(url)
    fun resourceIndex(url: String): Int? = automatic[url]
    fun isFragment(url: String): Boolean = url.startsWith("$documentUrl#")

    fun assemble(parts: List<JsonElement>): String {
        require(parts.size in 1..50_001)
        var markupBytes = 0L
        var bytes = 0L
        return buildString {
            parts.forEach { part ->
                val value = if (part is JsonPrimitive && part.isString) {
                    part.content.also { markupBytes += it.toByteArray().size; require(markupBytes <= 2 * 1024 * 1024 + 8192) }
                } else {
                    require(part is JsonObject && part.keys == setOf("asset"))
                    val field = part["asset"]
                    require(field is JsonPrimitive && !field.isString)
                    val index = field.intOrNull ?: throw IllegalArgumentException("invalid resource")
                    require(article.assets.getOrNull(index)?.inlineImage == true)
                    assetUrl(index)
                }
                bytes += value.toByteArray().size
                require(bytes <= 8 * 1024 * 1024)
                append(value)
            }
        }
    }

    companion object {
        fun safeExternal(url: String): Boolean = runCatching {
            if (url.any { it.isWhitespace() || it.isISOControl() || it == '\\' }) return false
            val uri = URI(url)
            uri.scheme in setOf("http", "https") && !uri.host.isNullOrBlank() && uri.rawUserInfo == null &&
                uri.host != "articles.lam.invalid" && (uri.port == -1 || uri.port in 1..65535)
        }.getOrDefault(false)
    }
}
