package dev.carraes.lam.articles

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.BitmapFactory
import android.os.Handler
import android.os.Looper
import android.webkit.*
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LocalLifecycleOwner
import dev.carraes.lam.R
import dev.carraes.lam.ui.markdown.LinkDestinationDialog
import dev.carraes.lam.ui.markdown.openWebLink
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import java.io.ByteArrayInputStream
import java.util.concurrent.atomic.AtomicBoolean

@Composable
fun ArticleReader(state: ArticleReaderState, repository: ArticleRepository, onVisible: (LoadedArticle) -> Unit,
    onBack: () -> Unit, onReload: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    var destination by remember { mutableStateOf<String?>(null) }
    var missing by remember(state.document) { mutableStateOf(false) }
    var feedback by remember { mutableStateOf<Int?>(null) }
    var pending by remember { mutableStateOf<Pair<LoadedArticle, Int>?>(null) }
    var saving by remember { mutableStateOf(false) }
    var web by remember { mutableStateOf<WebView?>(null) }
    var textZoom by rememberSaveable { mutableIntStateOf(100) }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val selected = pending
        pending = null
        if (uri != null && selected != null) scope.launch {
            saving = true
            val (document, index) = selected
            val bytes = repository.asset(document.session, document.article, index, attachment = true)
            val succeeded = bytes != null && repository.isCurrent(document.session) && withContext(Dispatchers.IO) {
                runCatching { context.contentResolver.openOutputStream(uri, "wt")?.use { it.write(bytes) } != null }.getOrDefault(false)
            }
            feedback = if (succeeded) R.string.article_download_saved else R.string.article_download_failed
            saving = false
        }
    }
    Surface(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize().safeDrawingPadding()) {
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
            TextButton(onBack) { Text(stringResource(R.string.article_back)) }
            TextButton(onReload, enabled = !state.loading) { Text(stringResource(R.string.detail_refresh)) }
        }
        state.document?.let { document ->
            Text(document.article.title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 16.dp))
            Row(Modifier.horizontalScroll(rememberScrollState())) {
                TextButton({ textZoom = (textZoom - 10).coerceAtLeast(60); web?.settings?.textZoom = textZoom }) { Text(stringResource(R.string.article_zoom_out)) }
                TextButton({ textZoom = (textZoom + 10).coerceAtMost(200); web?.settings?.textZoom = textZoom }) { Text(stringResource(R.string.article_zoom_in)) }
                document.article.assets.forEachIndexed { index, asset ->
                    if (asset.disposition == "attachment") TextButton({ pending = document to index; save.launch(asset.path.substringAfterLast('/')) }, enabled = !saving) {
                        Text(stringResource(R.string.article_download, asset.path))
                    }
                }
            }
            if (document.cached) Text(stringResource(R.string.article_cached), Modifier.padding(horizontal = 16.dp))
            if (missing) Text(stringResource(R.string.article_image_missing), Modifier.padding(horizontal = 16.dp))
            state.readResult?.let { Text(stringResource(readMessage(it)), Modifier.padding(horizontal = 16.dp)) }
            feedback?.let { Text(stringResource(it), Modifier.padding(horizontal = 16.dp)) }
            key(document, state.renderGeneration) {
                var committed by remember { mutableStateOf(false) }
                // A document committed in the background becomes read only after the reader resumes.
                DisposableEffect(lifecycle, committed) {
                    fun visible() {
                        if (committed && lifecycle.currentState.isAtLeast(Lifecycle.State.RESUMED) && web?.isShown == true) onVisible(document)
                    }
                    val observer = androidx.lifecycle.LifecycleEventObserver { _, event -> if (event == Lifecycle.Event.ON_RESUME) visible() }
                    lifecycle.addObserver(observer)
                    onDispose { lifecycle.removeObserver(observer) }
                }
                AndroidView(factory = { ctx ->
                    createArticleWebView(ctx, document, repository, { committed = true }, { missing = true }, { destination = it }).also {
                        it.settings.textZoom = textZoom
                        web = it
                    }
                }, modifier = Modifier.fillMaxWidth().weight(1f), onRelease = { view ->
                    view.stopLoading(); view.destroy(); if (web === view) web = null
                })
            }
        }
        if (state.loading) CircularProgressIndicator(Modifier.padding(16.dp))
        if (state.failed) Text(stringResource(R.string.article_failed), Modifier.padding(16.dp))
    }
    }
    destination?.let { LinkDestinationDialog(it, { destination = null }, { url ->
        ArticleResourcePolicy.safeExternal(url) && openWebLink(context, url)
    }) }
}

internal fun readMessage(result: ReadResult) = when (result) {
    ReadResult.SAVED -> R.string.article_read_saved
    ReadResult.CONFLICT -> R.string.article_conflict
    ReadResult.FAILED -> R.string.article_read_failed
}

/** No loadUrl call ever receives an API origin or credential. All network paths fail closed. */
@SuppressLint("SetJavaScriptEnabled")
internal fun createArticleWebView(context: Context, document: LoadedArticle, repository: ArticleRepository,
    onVisible: () -> Unit, onMissingImage: () -> Unit, onExternal: (String) -> Unit): WebView {
    val policy = ArticleResourcePolicy(document.session.epoch, document.article)
    val served = AtomicBoolean(false)
    val notified = AtomicBoolean(false)
    val handler = Handler(Looper.getMainLooper())
    val resources = ArticleImages(document, repository) { handler.post(onMissingImage) }
    return WebView(context).apply {
        contentDescription = context.getString(R.string.article_content)
        settings.apply {
            javaScriptEnabled = false
            allowFileAccess = false
            allowContentAccess = false
            domStorageEnabled = false
            blockNetworkLoads = true
            mixedContentMode = WebSettings.MIXED_CONTENT_NEVER_ALLOW
            cacheMode = WebSettings.LOAD_NO_CACHE
            setSupportZoom(true)
            builtInZoomControls = true
            displayZoomControls = false
            useWideViewPort = true
            loadWithOverviewMode = true
            safeBrowsingEnabled = true
            setSupportMultipleWindows(false)
        }
        CookieManager.getInstance().setAcceptThirdPartyCookies(this, false)
        setDownloadListener { _, _, _, _, _ -> /* Native save controls are the only download entry point. */ }
        webViewClient = object : WebViewClient() {
            override fun shouldInterceptRequest(view: WebView, request: WebResourceRequest): WebResourceResponse {
                if (!repository.isCurrent(document.session) || request.method != "GET" || request.isRedirect) return denied()
                val url = request.url.toString()
                if (request.isForMainFrame && url == policy.documentUrl) {
                    served.set(true)
                    return WebResourceResponse("text/html", "UTF-8", 200, "OK", mapOf(
                        "Content-Security-Policy" to "default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src https://articles.lam.invalid; base-uri 'none'; form-action 'none'; frame-src 'none'",
                        "Cache-Control" to "no-store", "Referrer-Policy" to "no-referrer", "X-Content-Type-Options" to "nosniff",
                    ), ByteArrayInputStream(document.html.toByteArray()))
                }
                if (!request.isForMainFrame) policy.resourceIndex(url)?.let { return resources.response(it) }
                return denied()
            }
            override fun shouldOverrideUrlLoading(view: WebView, request: WebResourceRequest): Boolean {
                val url = request.url.toString()
                if (repository.isCurrent(document.session) && request.isForMainFrame && policy.isFragment(url) && !request.isRedirect) return false
                if (repository.isCurrent(document.session) && request.isForMainFrame && request.hasGesture() && !request.isRedirect &&
                    ArticleResourcePolicy.safeExternal(url)) onExternal(url)
                return true
            }
            override fun onPageCommitVisible(view: WebView, url: String) {
                if (url == policy.documentUrl && served.get() && repository.isCurrent(document.session) && notified.compareAndSet(false, true)) onVisible()
            }
            override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                if (request.isForMainFrame) served.set(false)
            }
            override fun onReceivedSslError(view: WebView, handler: SslErrorHandler, error: android.net.http.SslError) { handler.cancel() }
        }
        loadUrl(policy.documentUrl)
    }
}

private fun denied() = WebResourceResponse("text/plain", "UTF-8", 403, "Blocked", mapOf("Cache-Control" to "no-store"), ByteArrayInputStream(ByteArray(0)))

private class ArticleImages(private val document: LoadedArticle, private val repository: ArticleRepository, private val missing: () -> Unit) {
    private val cached = mutableMapOf<Int, ByteArray?>()
    private var pixels = 0L
    private var encodedBytes = 0L
    @Synchronized fun response(index: Int): WebResourceResponse {
        if (!repository.isCurrent(document.session)) return denied()
        if (!cached.containsKey(index)) {
            val data = runBlocking { repository.asset(document.session, document.article, index, attachment = false) }
            val safe = data?.takeIf { image ->
                val options = BitmapFactory.Options().apply { inJustDecodeBounds = true }
                BitmapFactory.decodeByteArray(image, 0, image.size, options)
                val count = options.outWidth.toLong() * options.outHeight
                if (options.outWidth !in 1..8192 || options.outHeight !in 1..8192 || count > 16_000_000 ||
                    pixels + count > 32_000_000 || encodedBytes + image.size > 50L * 1024 * 1024) false
                else BitmapFactory.decodeByteArray(image, 0, image.size)?.let { bitmap ->
                    bitmap.recycle(); pixels += count; encodedBytes += image.size; true
                } ?: false
            }
            cached[index] = safe
            if (safe == null) missing()
        }
        if (!repository.isCurrent(document.session)) return denied()
        val bytes = cached[index]
        if (bytes == null) return WebResourceResponse("image/svg+xml", "UTF-8", ByteArrayInputStream(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"180\" height=\"48\"><rect width=\"180\" height=\"48\" fill=\"#eee\"/><text x=\"8\" y=\"28\" fill=\"#555\">Image unavailable</text></svg>".toByteArray()))
        return WebResourceResponse(document.article.assets[index].mediaType, null, 200, "OK", mapOf("Cache-Control" to "no-store", "X-Content-Type-Options" to "nosniff"), ByteArrayInputStream(bytes))
    }
}
