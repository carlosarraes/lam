package dev.carraes.lam.items

interface LamApi {
    suspend fun listOpenItems(): List<ItemDto>

    suspend fun getItem(id: String): ItemDto

    suspend fun getHistory(
        query: String? = null,
        priority: PriorityDto? = null,
        type: ItemTypeDto? = null,
        cursor: String? = null,
        limit: Int = 50,
    ): HistoryPageDto

    suspend fun replyChoice(id: String, choice: String): ItemDto

    suspend fun replyText(id: String, text: String): ItemDto

    suspend fun dismiss(id: String): ItemDto

    suspend fun setCheck(id: String, index: Int, done: Boolean): ItemDto

    suspend fun getDevice(): DeviceRegistrationDto

    suspend fun updateDevice(update: DeviceUpdateDto): DeviceRegistrationDto

    suspend fun revokeDevice(): DeviceSummaryDto

    suspend fun claimPairing(
        sessionId: String,
        request: PairingClaimRequestDto,
    ): PairingClaimResponseDto
}
