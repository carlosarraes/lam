package dev.carraes.lam.items

data class Item(
    val id: String,
    val name: String?,
    val title: String,
    val body: String,
    val sourceHost: String,
    val sourceProject: String,
    val priority: PriorityDto,
    val choices: List<String>,
    val checks: List<CheckDto>,
    val recommendation: String?,
    val recommendedChoice: String?,
    val link: String,
    val status: StatusDto,
    val responseChoice: String?,
    val responseText: String?,
    val responseBy: ResponseByDto?,
    val createdAt: String,
    val resolvedAt: String?,
    val expiresAt: String?,
    val version: Long,
) {
    val agentDisplay: String get() = name?.takeIf(String::isNotBlank) ?: "$sourceHost:$sourceProject"
}
