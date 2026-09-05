package dev.carraes.lam.ui.markdown

import android.content.ActivityNotFoundException
import android.content.Context
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.platform.UriHandler
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextDirection
import androidx.compose.ui.text.style.TextOverflow
import androidx.core.net.toUri
import com.mikepenz.markdown.compose.MarkdownElement
import com.mikepenz.markdown.compose.components.markdownComponents
import com.mikepenz.markdown.compose.elements.MarkdownTable
import com.mikepenz.markdown.compose.elements.MarkdownTableHeader
import com.mikepenz.markdown.compose.elements.MarkdownTableRow
import com.mikepenz.markdown.m3.Markdown
import com.mikepenz.markdown.m3.markdownTypography
import com.mikepenz.markdown.model.NoOpImageTransformerImpl
import com.mikepenz.markdown.model.markdownAnnotator
import dev.carraes.lam.R
import java.net.URI
import org.intellij.markdown.MarkdownElementTypes
import org.intellij.markdown.MarkdownTokenTypes
import org.intellij.markdown.ast.getTextInNode

@Composable
fun LamMarkdown(content: String, onLink: (String) -> Unit, modifier: Modifier = Modifier) {
    val linkHandler = remember(onLink) { object : UriHandler { override fun openUri(uri: String) = onLink(uri) } }
    val type = MaterialTheme.typography
    val body = type.bodyLarge.copy(textDirection = TextDirection.Content)
    val components = markdownComponents(
        table = { model ->
            MarkdownTable(model.content, model.node, model.typography.table,
                headerBlock = { text, node, width, style ->
                    MarkdownTableHeader(text, node, width, style, maxLines = Int.MAX_VALUE, overflow = TextOverflow.Clip)
                },
                rowBlock = { text, node, width, style ->
                    MarkdownTableRow(text, node, width, style, maxLines = Int.MAX_VALUE, overflow = TextOverflow.Clip)
                })
        },
        custom = { type, model ->
            if (type == MarkdownElementTypes.HTML_BLOCK) {
                Text(model.node.getTextInNode(model.content).toString(), style = model.typography.text)
            } else if (type != MarkdownElementTypes.LINK_DEFINITION) {
                model.node.children.forEach { MarkdownElement(it, com.mikepenz.markdown.compose.LocalMarkdownComponents.current, model.content) }
            }
        },
    )
    CompositionLocalProvider(LocalUriHandler provides linkHandler) {
        Markdown(content, modifier = modifier.fillMaxWidth(), imageTransformer = NoOpImageTransformerImpl(),
            typography = markdownTypography(
                h1 = type.headlineSmall.copy(textDirection = TextDirection.Content),
                h2 = type.titleLarge.copy(textDirection = TextDirection.Content),
                h3 = type.titleMedium.copy(textDirection = TextDirection.Content),
                h4 = type.titleMedium.copy(textDirection = TextDirection.Content),
                h5 = type.titleSmall.copy(textDirection = TextDirection.Content),
                h6 = type.titleSmall.copy(textDirection = TextDirection.Content),
                text = body, paragraph = body, ordered = body, bullet = body, list = body, table = body,
                quote = type.bodyMedium.copy(fontStyle = FontStyle.Italic, textDirection = TextDirection.Content),
                code = type.bodyMedium.copy(fontFamily = FontFamily.Monospace, textDirection = TextDirection.Ltr),
            ),
            annotator = markdownAnnotator { source, node ->
                if (node.type == MarkdownTokenTypes.HTML_TAG) {
                    append(node.getTextInNode(source)); true
                } else false
            },
            components = components,
            error = { Text(content) })
    }
}

fun isSupportedWebLink(destination: String): Boolean {
    if (destination.any { it.isISOControl() }) return false
    val uri = runCatching { URI(destination) }.getOrNull() ?: return false
    return (uri.scheme.equals("https", true) || uri.scheme.equals("http", true)) &&
        !uri.host.isNullOrBlank() && uri.rawUserInfo == null
}

fun openWebLink(context: Context, destination: String): Boolean {
    if (!isSupportedWebLink(destination)) return false
    return try {
        CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, destination.toUri())
        true
    } catch (_: ActivityNotFoundException) {
        false
    } catch (_: SecurityException) {
        false
    }
}

@Composable
fun LinkDestinationDialog(destination: String, onDismiss: () -> Unit, onOpen: (String) -> Boolean) {
    var failed by remember(destination) { mutableStateOf(false) }
    val supported = isSupportedWebLink(destination)
    AlertDialog(onDismissRequest = onDismiss, title = { Text(stringResource(R.string.detail_open_link)) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text(destination)
                if (!supported) Text(stringResource(R.string.detail_unsupported_link))
                if (failed) Text(stringResource(R.string.detail_browser_unavailable))
            }
        },
        confirmButton = { TextButton(onClick = { if (onOpen(destination)) onDismiss() else failed = true }, enabled = supported) {
            Text(stringResource(R.string.detail_open_browser))
        } },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.pairing_cancel)) } })
}
