package dev.carraes.lam

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.background
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import dev.carraes.lam.ui.theme.Amber
import dev.carraes.lam.ui.theme.Graphite
import dev.carraes.lam.ui.theme.LamTheme

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            LamTheme {
                RequestsSmokeSurface()
            }
        }
    }
}

@Composable
private fun RequestsSmokeSurface() {
    Surface(
        modifier = Modifier
            .fillMaxSize()
            .testTag("requests-surface"),
        color = Graphite,
    ) {
        Column(modifier = Modifier.padding(horizontal = 24.dp, vertical = 28.dp)) {
            Text(
                text = stringResource(R.string.requests_title),
                style = androidx.compose.material3.MaterialTheme.typography.headlineLarge,
            )
            Spacer(modifier = Modifier.height(12.dp))
            Spacer(
                modifier = Modifier
                    .width(32.dp)
                    .height(2.dp)
                    .clip(androidx.compose.foundation.shape.CircleShape)
                    .background(Amber),
            )
        }
    }
}
