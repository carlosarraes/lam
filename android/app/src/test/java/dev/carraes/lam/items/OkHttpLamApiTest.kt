package dev.carraes.lam.items

import java.util.concurrent.TimeUnit
import kotlinx.coroutines.test.runTest
import mockwebserver3.MockResponse
import mockwebserver3.MockWebServer
import mockwebserver3.SocketEffect
import okhttp3.HttpUrl
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class OkHttpLamApiTest {
    private lateinit var server: MockWebServer
    private lateinit var api: LamApi

    @Before
    fun setUp() {
        server = MockWebServer()
        server.start()
        api = api(server.url("/api/"))
    }

    @After
    fun tearDown() {
        server.close()
    }

    @Test
    fun `open listing injects the current bearer and decodes every item field`() = runTest {
        server.enqueue(jsonResponse(body = "[$ITEM_JSON]"))

        val items = api.listOpenItems()

        assertEquals(listOf(ITEM), items)
        server.takeRequest().apply {
            assertEquals("GET", method)
            assertEquals("/api/items?status=open", target)
            assertEquals("Bearer device-credential", headers["Authorization"])
        }
    }

    @Test
    fun `bearer is read for each authenticated request instead of captured at construction`() = runTest {
        var credential = "first-credential"
        api = OkHttpLamApi(server.url("/"), credentialProvider = { credential })
        server.enqueue(jsonResponse(body = "[]"))
        server.enqueue(jsonResponse(body = "[]"))

        api.listOpenItems()
        credential = "second-credential"
        api.listOpenItems()

        assertEquals("Bearer first-credential", server.takeRequest().headers["Authorization"])
        assertEquals("Bearer second-credential", server.takeRequest().headers["Authorization"])
    }

    @Test
    fun `missing credential leaves authenticated request without an authorization header`() = runTest {
        api = OkHttpLamApi(server.url("/"), credentialProvider = { null })
        server.enqueue(jsonResponse(401, "{\"error\":\"unauthorized\"}"))

        expectError<ApiError.Unauthorized> { api.listOpenItems() }

        assertNull(server.takeRequest().headers["Authorization"])
    }

    @Test
    fun `get encodes the item id as one path segment`() = runTest {
        server.enqueue(jsonResponse(body = ITEM_JSON))

        assertEquals(ITEM, api.getItem("item /?#"))

        assertEquals("/api/items/item%20%2F%3F%23", server.takeRequest().target)
    }

    @Test
    fun `history sends encoded search priority type cursor and limit and decodes its page`() = runTest {
        server.enqueue(jsonResponse(body = "{\"items\":[$ITEM_JSON],\"next_cursor\":\"next+/=\"}"))

        val page = api.getHistory(
            query = "space &/ unicode ☃",
            priority = PriorityDto.CRITICAL,
            type = ItemTypeDto.CHECKLIST,
            cursor = "opaque+/=",
            limit = 25,
        )

        assertEquals(HistoryPageDto(listOf(ITEM), "next+/="), page)
        assertEquals(
            "/api/history?q=space%20%26%2F%20unicode%20%E2%98%83&priority=critical&type=checklist&cursor=opaque%2B%2F%3D&limit=25",
            server.takeRequest().target,
        )
    }

    @Test
    fun `choice reply posts the deployed resolve shape once`() = runTest {
        server.enqueue(jsonResponse(body = RESOLVED_ITEM_JSON))

        assertEquals(RESOLVED_ITEM, api.replyChoice("abc 12", "Ship / now"))

        server.takeRequest().apply {
            assertEquals("POST", method)
            assertEquals("/api/items/abc%2012/resolve", target)
            assertJsonBody("{\"choice\":\"Ship / now\"}")
        }
    }

    @Test
    fun `free reply posts the deployed resolve shape once`() = runTest {
        server.enqueue(jsonResponse(body = RESOLVED_ITEM_JSON))

        api.replyText("abc12", "A multiline\nanswer")

        server.takeRequest().apply {
            assertEquals("POST", method)
            assertEquals("/api/items/abc12/resolve", target)
            assertJsonBody("{\"text\":\"A multiline\\nanswer\"}")
        }
    }

    @Test
    fun `dismiss sends one empty post`() = runTest {
        server.enqueue(jsonResponse(body = DISMISSED_ITEM_JSON))

        assertEquals(DISMISSED_ITEM, api.dismiss("abc12"))

        server.takeRequest().apply {
            assertEquals("POST", method)
            assertEquals("/api/items/abc12/dismiss", target)
            assertEquals(0L, bodySize)
        }
    }

    @Test
    fun `set check posts the index and boolean`() = runTest {
        server.enqueue(jsonResponse(body = ITEM_JSON))

        api.setCheck("abc12", 1, true)

        server.takeRequest().apply {
            assertEquals("POST", method)
            assertEquals("/api/items/abc12/checks/1", target)
            assertJsonBody("{\"done\":true}")
        }
    }

    @Test
    fun `device self read decodes only safe registration fields`() = runTest {
        server.enqueue(jsonResponse(body = DEVICE_JSON))

        assertEquals(DEVICE, api.getDevice())

        server.takeRequest().apply {
            assertEquals("GET", method)
            assertEquals("/api/device", target)
        }
    }

    @Test
    fun `device self update sends the complete public registration update`() = runTest {
        server.enqueue(jsonResponse(body = DEVICE_JSON))
        val update = DeviceUpdateDto(
            name = "Carlos phone",
            fcmToken = null,
            appVersion = "0.1.0",
            androidVersion = "16",
        )

        assertEquals(DEVICE, api.updateDevice(update))

        server.takeRequest().apply {
            assertEquals("PATCH", method)
            assertEquals("/api/device", target)
            assertJsonBody(
                "{\"name\":\"Carlos phone\",\"fcm_token\":null,\"app_version\":\"0.1.0\",\"android_version\":\"16\"}",
            )
        }
    }

    @Test
    fun `device self revoke decodes the revocation summary`() = runTest {
        server.enqueue(jsonResponse(body = REVOKED_DEVICE_JSON))

        assertEquals(REVOKED_DEVICE, api.revokeDevice())

        server.takeRequest().apply {
            assertEquals("DELETE", method)
            assertEquals("/api/device", target)
            assertEquals(0L, bodySize)
        }
    }

    @Test
    fun `pairing claim omits bearer and sends the exact public claim contract`() = runTest {
        server.enqueue(jsonResponse(201, "{\"credential\":\"permanent-device-credential\",\"device\":$DEVICE_JSON}"))
        val request = PairingClaimRequestDto(
            secret = "one-time-secret",
            name = "Carlos phone",
            fcmToken = "fcm-token",
            appVersion = "0.1.0",
            androidVersion = "16",
        )

        val claimed = api.claimPairing("session /?#", request)

        assertEquals(PairingClaimResponseDto("permanent-device-credential", DEVICE), claimed)
        server.takeRequest().apply {
            assertEquals("POST", method)
            assertEquals("/api/pairings/session%20%2F%3F%23/claim", target)
            assertNull(headers["Authorization"])
            assertJsonBody(
                "{\"secret\":\"one-time-secret\",\"name\":\"Carlos phone\",\"fcm_token\":\"fcm-token\",\"app_version\":\"0.1.0\",\"android_version\":\"16\"}",
            )
        }
    }

    @Test
    fun `status responses map to typed errors with bounded safe bodies`() = runTest {
        val cases = listOf(
            400 to ApiError.Validation::class.java,
            401 to ApiError.Unauthorized::class.java,
            403 to ApiError.Forbidden::class.java,
            409 to ApiError.AlreadyClosed::class.java,
            404 to ApiError.Server::class.java,
            500 to ApiError.Server::class.java,
        )

        cases.forEachIndexed { index, (status, type) ->
            val body = if (index == 0) "{\"error\":\"${"x".repeat(4_500)} device-credential\"}" else "{\"error\":\"status-$status\"}"
            server.enqueue(jsonResponse(status, body))
            val error = expectError<ApiError> { api.getItem("status-$status") }
            assertTrue("$status mapped to ${error.javaClass}", type.isInstance(error))
            assertEquals(status, error.statusCode)
            assertTrue(error.responseBody.orEmpty().length <= ApiError.MAX_RESPONSE_BODY_CHARACTERS)
            assertFalse(error.responseBody.orEmpty().contains("device-credential"))
        }
    }

    @Test
    fun `pairing conflict preserves its safe machine code without retaining the secret`() = runTest {
        server.enqueue(jsonResponse(409, "{\"error\":\"expired\",\"echo\":\"one-time-secret\"}"))

        val error = expectError<ApiError.AlreadyClosed> {
            api.claimPairing(
                "session",
                PairingClaimRequestDto("one-time-secret", "Phone", null, "0.1.0", "16"),
            )
        }

        assertTrue(error.responseBody.orEmpty().contains("expired"))
        assertFalse(error.responseBody.orEmpty().contains("one-time-secret"))
    }

    @Test
    fun `request and response DTO strings redact credentials secrets and push tokens`() {
        val claim = PairingClaimRequestDto("one-time-secret", "Phone", "fcm-secret", "0.1.0", "16")
        val claimed = PairingClaimResponseDto("device-credential", DEVICE)
        val update = DeviceUpdateDto("Phone", "fcm-secret", "0.1.0", "16")

        assertFalse(claim.toString().contains("one-time-secret"))
        assertFalse(claim.toString().contains("fcm-secret"))
        assertFalse(claimed.toString().contains("device-credential"))
        assertFalse(update.toString().contains("fcm-secret"))
    }

    @Test
    fun `malformed claim response cannot copy a returned credential into an error`() = runTest {
        server.enqueue(jsonResponse(201, "{\"credential\":\"returned-private-token\",\"device\":{}}"))

        val error = expectError<ApiError.Server> {
            api.claimPairing(
                "session",
                PairingClaimRequestDto("one-time-secret", "Phone", null, "0.1.0", "16"),
            )
        }

        assertFalse(error.responseBody.orEmpty().contains("returned-private-token"))
        assertTrue(error.responseBody.orEmpty().contains("[redacted]"))
    }

    @Test
    fun `a read retries once after a connection failure and then returns the response`() = runTest {
        server.enqueue(disconnectResponse())
        server.enqueue(jsonResponse(body = "[]"))

        assertEquals(emptyList<ItemDto>(), api.listOpenItems())

        assertEquals(2, server.requestCount)
    }

    @Test
    fun `a read stops after the single connection retry`() = runTest {
        server.enqueue(disconnectResponse())
        server.enqueue(disconnectResponse())
        server.enqueue(jsonResponse(body = "[]"))

        expectError<ApiError.Transport> { api.listOpenItems() }

        assertEquals(2, server.requestCount)
    }

    @Test
    fun `every mutation is sent at most once after a connection failure`() = runTest {
        val mutations: List<suspend (LamApi) -> Unit> = listOf(
            { it.replyChoice("id", "yes") },
            { it.replyText("id", "answer") },
            { it.dismiss("id") },
            { it.setCheck("id", 0, true) },
            { it.updateDevice(DeviceUpdateDto("Phone", null, "0.1.0", "16")) },
            { it.revokeDevice() },
            { it.claimPairing("session", PairingClaimRequestDto("secret", "Phone", null, "0.1.0", "16")) },
        )

        mutations.forEachIndexed { index, mutation ->
            val isolatedServer = MockWebServer()
            isolatedServer.start()
            try {
                isolatedServer.enqueue(disconnectResponse())
                isolatedServer.enqueue(jsonResponse(body = ITEM_JSON))
                val isolatedApi = api(isolatedServer.url("/"))

                expectError<ApiError.Transport> { mutation(isolatedApi) }

                assertEquals("mutation $index", 1, isolatedServer.requestCount)
                assertNotNull("mutation $index was not observed", isolatedServer.takeRequest(100, TimeUnit.MILLISECONDS))
                assertNull("mutation $index retried", isolatedServer.takeRequest(100, TimeUnit.MILLISECONDS))
            } finally {
                isolatedServer.close()
            }
        }
    }

    private fun api(baseUrl: HttpUrl): LamApi = OkHttpLamApi(
        baseUrl = baseUrl,
        credentialProvider = { "device-credential" },
    )

    private fun jsonResponse(code: Int = 200, body: String): MockResponse = MockResponse.Builder()
        .code(code)
        .addHeader("Content-Type", "application/json")
        .body(body)
        .build()

    private fun disconnectResponse(): MockResponse = MockResponse.Builder()
        .onRequestStart(SocketEffect.CloseSocket())
        .build()

    private inline fun <reified T : ApiError> expectError(block: () -> Unit): T = try {
        block()
        fail("Expected ${T::class.java.simpleName}")
        throw AssertionError("unreachable")
    } catch (error: ApiError) {
        if (error !is T) throw error
        error
    }

    private fun mockwebserver3.RecordedRequest.assertJsonBody(expected: String) {
        assertEquals("application/json; charset=utf-8", headers["Content-Type"])
        assertEquals(expected, body?.utf8().orEmpty())
    }

    companion object {
        private val ITEM = ItemDto(
            id = "abc12",
            name = null,
            title = "Release?",
            body = "Review **this**",
            sourceHost = "workstation",
            sourceProject = "lam",
            priority = PriorityDto.CRITICAL,
            choices = listOf("ship", "hold"),
            checks = listOf(CheckDto("Smoke test", false, null)),
            recommendation = "Ship after smoke test.",
            recommendedChoice = "ship",
            link = "https://example.test/pr/1",
            status = StatusDto.OPEN,
            responseChoice = null,
            responseText = null,
            responseBy = null,
            createdAt = "2026-09-03T12:00:00.000Z",
            resolvedAt = null,
            expiresAt = "2026-09-03T13:00:00.000Z",
            version = 7,
        )
        private val RESOLVED_ITEM = ITEM.copy(
            status = StatusDto.RESOLVED,
            responseChoice = "ship",
            responseBy = ResponseByDto.PHONE,
            resolvedAt = "2026-09-03T12:05:00.000Z",
            version = 8,
        )
        private val DISMISSED_ITEM = ITEM.copy(
            status = StatusDto.DISMISSED,
            responseBy = ResponseByDto.PHONE,
            resolvedAt = "2026-09-03T12:05:00.000Z",
            version = 8,
        )
        private val DEVICE = DeviceRegistrationDto(
            id = "device-1",
            name = "Carlos phone",
            appVersion = "0.1.0",
            androidVersion = "16",
            createdAt = "2026-09-03T11:00:00.000Z",
            lastSeenAt = "2026-09-03T12:00:00.000Z",
            pushRegistered = true,
        )
        private val REVOKED_DEVICE = DeviceSummaryDto(
            id = "device-1",
            name = "Carlos phone",
            appVersion = "0.1.0",
            androidVersion = "16",
            createdAt = "2026-09-03T11:00:00.000Z",
            lastSeenAt = "2026-09-03T12:00:00.000Z",
            pushRegistered = false,
            revokedAt = "2026-09-03T12:10:00.000Z",
        )

        private const val ITEM_JSON = """{"id":"abc12","name":null,"title":"Release?","body":"Review **this**","source_host":"workstation","source_project":"lam","priority":"critical","choices":["ship","hold"],"checks":[{"label":"Smoke test","done":false,"at":null}],"recommendation":"Ship after smoke test.","recommended_choice":"ship","link":"https://example.test/pr/1","status":"open","response_choice":null,"response_text":null,"response_by":null,"created_at":"2026-09-03T12:00:00.000Z","resolved_at":null,"expires_at":"2026-09-03T13:00:00.000Z","version":7}"""
        private const val RESOLVED_ITEM_JSON = """{"id":"abc12","name":null,"title":"Release?","body":"Review **this**","source_host":"workstation","source_project":"lam","priority":"critical","choices":["ship","hold"],"checks":[{"label":"Smoke test","done":false,"at":null}],"recommendation":"Ship after smoke test.","recommended_choice":"ship","link":"https://example.test/pr/1","status":"resolved","response_choice":"ship","response_text":null,"response_by":"phone","created_at":"2026-09-03T12:00:00.000Z","resolved_at":"2026-09-03T12:05:00.000Z","expires_at":"2026-09-03T13:00:00.000Z","version":8}"""
        private const val DISMISSED_ITEM_JSON = """{"id":"abc12","name":null,"title":"Release?","body":"Review **this**","source_host":"workstation","source_project":"lam","priority":"critical","choices":["ship","hold"],"checks":[{"label":"Smoke test","done":false,"at":null}],"recommendation":"Ship after smoke test.","recommended_choice":"ship","link":"https://example.test/pr/1","status":"dismissed","response_choice":null,"response_text":null,"response_by":"phone","created_at":"2026-09-03T12:00:00.000Z","resolved_at":"2026-09-03T12:05:00.000Z","expires_at":"2026-09-03T13:00:00.000Z","version":8}"""
        private const val DEVICE_JSON = """{"id":"device-1","name":"Carlos phone","app_version":"0.1.0","android_version":"16","created_at":"2026-09-03T11:00:00.000Z","last_seen_at":"2026-09-03T12:00:00.000Z","push_registered":true}"""
        private const val REVOKED_DEVICE_JSON = """{"id":"device-1","name":"Carlos phone","app_version":"0.1.0","android_version":"16","created_at":"2026-09-03T11:00:00.000Z","last_seen_at":"2026-09-03T12:00:00.000Z","push_registered":false,"revoked_at":"2026-09-03T12:10:00.000Z"}"""
    }
}
