package dev.carraes.lam.items

import androidx.room.TypeConverter
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json

class ItemConverters {
    @TypeConverter fun encodeChoices(value: List<String>): String = Json.encodeToString(value)
    @TypeConverter fun decodeChoices(value: String): List<String> = Json.decodeFromString(value)
    @TypeConverter fun encodeChecks(value: List<CheckDto>): String = Json.encodeToString(value)
    @TypeConverter fun decodeChecks(value: String): List<CheckDto> = Json.decodeFromString(value)
}
