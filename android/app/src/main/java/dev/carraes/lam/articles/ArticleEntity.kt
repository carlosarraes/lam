package dev.carraes.lam.articles

import androidx.room.Entity
import kotlinx.serialization.encodeToString

@Entity(tableName = "articles", primaryKeys = ["account", "id"])
data class ArticleEntity(val account: String, val id: String, val json: String,
    val content: String? = null, val contentSha256: String? = null) {
    fun article(): Article = articleJson.decodeFromString(json)
    companion object {
        fun from(account: String, article: Article, content: String? = null) = ArticleEntity(account, article.id,
            articleJson.encodeToString(article), content, content?.let { sha256(it.toByteArray()) })
    }
}
