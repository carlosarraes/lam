package dev.carraes.lam.ui.detail

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.*
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import dev.carraes.lam.R
import dev.carraes.lam.items.Item
import dev.carraes.lam.ui.theme.Amber

@Composable
fun RecommendationCard(item: Item) {
    if (item.checks.isNotEmpty()) return
    Surface(Modifier.fillMaxWidth().testTag("recommendation"), shape = RoundedCornerShape(8.dp),
        color = Amber.copy(alpha = 0.07f), border = BorderStroke(1.dp, Amber.copy(alpha = 0.3f))) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(if (item.recommendation.isNullOrBlank()) R.string.request_no_recommendation else R.string.detail_recommendation),
                color = Amber, style = MaterialTheme.typography.labelLarge, modifier = Modifier.semantics { heading() })
            if (!item.recommendation.isNullOrBlank()) {
                item.recommendedChoice?.takeIf { it in item.choices }?.let { Text(it, style = MaterialTheme.typography.titleMedium) }
                Text(item.recommendation, style = MaterialTheme.typography.bodyMedium)
            }
        }
    }
}
