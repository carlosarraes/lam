package dev.carraes.lam.items

import java.time.Instant

object ItemMapper {
    fun toEntity(dto: ItemDto) = ItemEntity(dto, Instant.parse(dto.createdAt).toEpochMilli(),
        (if (dto.status == StatusDto.EXPIRED) dto.expiresAt else dto.resolvedAt)?.let { Instant.parse(it).toEpochMilli() })

    fun toItem(entity: ItemEntity): Item = entity.canonical.let {
        Item(
            id = it.id,
            name = it.name,
            title = it.title,
            body = it.body,
            sourceHost = it.sourceHost,
            sourceProject = it.sourceProject,
            priority = it.priority,
            choices = it.choices,
            checks = it.checks,
            recommendation = it.recommendation,
            recommendedChoice = it.recommendedChoice,
            link = it.link,
            status = it.status,
            responseChoice = it.responseChoice,
            responseText = it.responseText,
            responseBy = it.responseBy,
            createdAt = it.createdAt,
            resolvedAt = it.resolvedAt,
            expiresAt = it.expiresAt,
            version = it.version,
            kind = it.kind,
            seenAt = it.seenAt,
        )
    }
}
