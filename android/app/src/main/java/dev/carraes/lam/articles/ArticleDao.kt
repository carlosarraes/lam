package dev.carraes.lam.articles

import androidx.room.*
import dev.carraes.lam.items.LamDatabase

@Dao
interface ArticleDao {
    @Query("SELECT * FROM articles WHERE account = :account") suspend fun list(account: String): List<ArticleEntity>
    @Query("SELECT * FROM articles WHERE account = :account AND id = :id") suspend fun get(account: String, id: String): ArticleEntity?
    @Insert(onConflict = OnConflictStrategy.REPLACE) suspend fun put(row: ArticleEntity)
    @Query("DELETE FROM articles") suspend fun clear()
}

interface ArticleStorage {
    suspend fun list(account: String): List<ArticleEntity>
    suspend fun get(account: String, id: String): ArticleEntity?
    suspend fun save(rows: List<ArticleEntity>, current: () -> Boolean): Boolean
}

class RoomArticleStorage(private val database: LamDatabase) : ArticleStorage {
    private val dao = database.articleDao()
    override suspend fun list(account: String) = dao.list(account)
    override suspend fun get(account: String, id: String) = dao.get(account, id)
    override suspend fun save(rows: List<ArticleEntity>, current: () -> Boolean): Boolean = database.withTransaction {
        if (!current()) return@withTransaction false
        rows.forEach { row ->
            val previous = dao.get(row.account, row.id)
            dao.put(row.copy(json = if (previous != null && previous.article().version > row.article().version) previous.json else row.json,
                content = row.content ?: previous?.content, contentSha256 = row.contentSha256 ?: previous?.contentSha256))
        }
        true
    }
}
