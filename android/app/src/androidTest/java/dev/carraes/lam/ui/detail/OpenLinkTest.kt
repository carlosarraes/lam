package dev.carraes.lam.ui.detail

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import dev.carraes.lam.ui.markdown.openLink
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class OpenLinkTest {
    @Test fun appLinkUsesAViewIntentWithTheFullDestinationAndNormalizedScheme() {
        var launched: Intent? = null
        val context = object : ContextWrapper(ApplicationProvider.getApplicationContext<Context>()) {
            override fun startActivity(intent: Intent) { launched = intent }
        }

        assertTrue(openLink(context, "GROKBOT://app/v1/agent?id=agent-123"))
        assertEquals(Intent.ACTION_VIEW, launched?.action)
        assertEquals("grokbot://app/v1/agent?id=agent-123", launched?.dataString)
        assertNull(launched?.`package`)
        assertNull(launched?.component)
    }

    @Test fun missingAppReturnsFailureWithoutThrowing() {
        var attempts = 0
        val context = object : ContextWrapper(ApplicationProvider.getApplicationContext<Context>()) {
            override fun startActivity(intent: Intent) {
                attempts++
                throw ActivityNotFoundException()
            }
        }

        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            assertFalse(openLink(context, "grokbot://app/v1/agent?id=agent-123"))
        }
        assertEquals(1, attempts)
    }

    @Test fun unsafeLinksNeverLaunchAnActivity() {
        var attempts = 0
        val context = object : ContextWrapper(ApplicationProvider.getApplicationContext<Context>()) {
            override fun startActivity(intent: Intent) { attempts++ }
        }

        assertFalse(openLink(context, "javascript:alert(1)"))
        assertEquals(0, attempts)
    }
}
