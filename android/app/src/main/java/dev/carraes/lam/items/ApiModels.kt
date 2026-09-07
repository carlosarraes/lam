package dev.carraes.lam.items

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import androidx.room.ColumnInfo

@Serializable
enum class ItemKindDto {
    @SerialName("request") REQUEST,
    @SerialName("fyi") FYI,
}

@Serializable
enum class PriorityDto {
    @SerialName("low")
    LOW,

    @SerialName("normal")
    NORMAL,

    @SerialName("critical")
    CRITICAL,
}

@Serializable
enum class StatusDto {
    @SerialName("open")
    OPEN,

    @SerialName("resolved")
    RESOLVED,

    @SerialName("dismissed")
    DISMISSED,

    @SerialName("retracted")
    RETRACTED,

    @SerialName("expired")
    EXPIRED,
}

@Serializable
enum class ResponseByDto {
    @SerialName("phone")
    PHONE,

    @SerialName("cli")
    CLI,
}

@Serializable
enum class ItemTypeDto {
    @SerialName("plain")
    PLAIN,

    @SerialName("choice")
    CHOICE,

    @SerialName("checklist")
    CHECKLIST,
}

@Serializable
data class CheckDto(
    val label: String,
    val done: Boolean,
    val at: String?,
)

@Serializable
data class ItemDto(
    val id: String,
    val name: String? = null,
    val title: String,
    val body: String,
    @SerialName("source_host") val sourceHost: String,
    @SerialName("source_project") val sourceProject: String,
    val priority: PriorityDto,
    val choices: List<String>,
    val checks: List<CheckDto>,
    val recommendation: String? = null,
    @SerialName("recommended_choice") val recommendedChoice: String? = null,
    val link: String,
    val status: StatusDto,
    @SerialName("response_choice") val responseChoice: String?,
    @SerialName("response_text") val responseText: String?,
    @SerialName("response_by") val responseBy: ResponseByDto?,
    @SerialName("created_at") val createdAt: String,
    @SerialName("resolved_at") val resolvedAt: String?,
    @SerialName("expires_at") val expiresAt: String?,
    val version: Long,
    @ColumnInfo(defaultValue = "'REQUEST'") val kind: ItemKindDto = ItemKindDto.REQUEST,
    @SerialName("seen_at") val seenAt: String? = null,
)

@Serializable
data class HistoryPageDto(
    val items: List<ItemDto>,
    @SerialName("next_cursor") val nextCursor: String?,
)

@Serializable
data class DeviceRegistrationDto(
    val id: String,
    val name: String,
    @SerialName("app_version") val appVersion: String,
    @SerialName("android_version") val androidVersion: String,
    @SerialName("created_at") val createdAt: String,
    @SerialName("last_seen_at") val lastSeenAt: String?,
    @SerialName("push_registered") val pushRegistered: Boolean,
)

@Serializable
data class DeviceSummaryDto(
    val id: String,
    val name: String,
    @SerialName("app_version") val appVersion: String,
    @SerialName("android_version") val androidVersion: String,
    @SerialName("created_at") val createdAt: String,
    @SerialName("last_seen_at") val lastSeenAt: String?,
    @SerialName("push_registered") val pushRegistered: Boolean,
    @SerialName("revoked_at") val revokedAt: String?,
)

data class DeviceUpdateDto(
    val name: String? = null,
    val fcmToken: FcmTokenUpdate = FcmTokenUpdate.Unchanged,
    val appVersion: String? = null,
    val androidVersion: String? = null,
) {
    override fun toString(): String =
        "DeviceUpdateDto(name=$name, fcmToken=${fcmToken.redacted()}, appVersion=$appVersion, androidVersion=$androidVersion)"
}

sealed interface FcmTokenUpdate {
    data object Unchanged : FcmTokenUpdate

    data object Clear : FcmTokenUpdate

    data class Set(val value: String) : FcmTokenUpdate {
        override fun toString(): String = "Set(value=[redacted])"
    }
}

@Serializable
data class PairingClaimRequestDto(
    val secret: String,
    val name: String,
    @SerialName("fcm_token") val fcmToken: String?,
    @SerialName("app_version") val appVersion: String,
    @SerialName("android_version") val androidVersion: String,
) {
    override fun toString(): String =
        "PairingClaimRequestDto(secret=[redacted], name=$name, fcmToken=${fcmToken.redacted()}, appVersion=$appVersion, androidVersion=$androidVersion)"
}

@Serializable
data class PairingClaimResponseDto(
    val credential: String,
    val device: DeviceRegistrationDto,
) {
    override fun toString(): String = "PairingClaimResponseDto(credential=[redacted], device=$device)"
}

private fun String?.redacted(): String = if (this == null) "null" else "[redacted]"

private fun FcmTokenUpdate.redacted(): String = when (this) {
    FcmTokenUpdate.Unchanged -> "Unchanged"
    FcmTokenUpdate.Clear -> "Clear"
    is FcmTokenUpdate.Set -> toString()
}
