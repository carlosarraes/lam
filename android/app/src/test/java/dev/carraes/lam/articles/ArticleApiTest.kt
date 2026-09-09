package dev.carraes.lam.articles

import kotlinx.coroutines.test.runTest
import kotlinx.serialization.encodeToString
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.Assert.*
import org.junit.Test

class ArticleApiTest {
    @Test fun `day is encoded and all dates omits day`() = runTest {
        MockWebServer().use { server ->
            val api = OkHttpArticleApi(server.url("/"), { "fixture" })
            repeat(2) { server.enqueue(MockResponse().setBody("{\"items\":[],\"next_cursor\":null}")) }
            api.list("", "all", null, "2026-09-08")
            assertEquals("2026-09-08", server.takeRequest().requestUrl!!.queryParameter("day"))
            api.list("", "unread", null)
            assertNull(server.takeRequest().requestUrl!!.queryParameter("day"))
        }
    }
    @Test fun `wire paths queries and CAS use header credentials without redirect forwarding`() = runTest {
        MockWebServer().use { server ->
            MockWebServer().use { other ->
                var token: String? = "test-only-device"
                val api = OkHttpArticleApi(server.url("/"), { token })
                server.enqueue(MockResponse().setBody(articleJson.encodeToString(ArticlePage(listOf(articleFixture()), "next"))))
                assertEquals("next", api.list("a & b", "unread", "opaque/?").nextCursor)
                val listed = server.takeRequest()
                assertEquals("test-only-device", listed.getHeader("Authorization")?.removePrefix("Bearer "))
                assertEquals("/v2/articles", listed.requestUrl!!.encodedPath)
                assertEquals("a & b", listed.requestUrl!!.queryParameter("q"))
                assertEquals("opaque/?", listed.requestUrl!!.queryParameter("cursor"))
                server.enqueue(MockResponse().setBody(articleJson.encodeToString(articleFixture(version = 4, read = true))))
                assertEquals(4L, api.setRead(articleFixture().id, true, 3).version)
                val write = server.takeRequest()
                assertEquals("PUT", write.method)
                assertEquals("{\"read\":true,\"version\":3}", write.body.readUtf8())
                server.enqueue(MockResponse().setResponseCode(302).setHeader("Location", other.url("/leak")))
                try { api.asset(articleFixture().id, 1, 4); fail("redirect accepted") } catch (error: ArticleHttpException) { assertEquals(302, error.status) }
                assertEquals(0, other.requestCount)
                token = null
                try { api.get(articleFixture().id); fail("missing credential accepted") } catch (error: ArticleHttpException) { assertEquals(401, error.status) }
                assertEquals(3, server.requestCount)
            }
        }
    }

    @Test fun `chunked resources cannot exceed declared bound`() = runTest {
        MockWebServer().use { server ->
            val api = OkHttpArticleApi(server.url("/"), { "fixture" })
            server.enqueue(MockResponse().setChunkedBody("12345", 1))
            try { api.asset(articleFixture().id, 1, 4); fail("oversize accepted") } catch (_: java.io.IOException) { }
        }
    }
}
