package dev.carraes.lam.items

import androidx.room.Embedded
import androidx.room.Entity
import androidx.room.Index

@Entity(tableName = "items", primaryKeys = ["id"], indices = [Index("status"), Index("priority"), Index("createdAt"), Index("effectiveClosureAt")])
data class ItemEntity(
    @Embedded val canonical: ItemDto,
    val createdAtEpoch: Long,
    val effectiveClosureAt: Long?,
    val optimisticTag: String? = null,
)
