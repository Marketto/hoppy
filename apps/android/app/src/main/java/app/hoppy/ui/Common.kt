package app.hoppy.ui

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.graphics.drawable.Drawable
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.LayerDrawable
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.util.TypedValue
import android.view.LayoutInflater
import android.view.ViewGroup
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import com.google.android.material.color.MaterialColors
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.Peer
import app.hoppy.core.Source
import app.hoppy.databinding.ItemPeerBinding
import app.hoppy.security.Security
import app.hoppy.ui.theme.HoppyTheme
import app.hoppy.ui.theme.HostAware
import app.hoppy.ui.theme.ThemeFonts
import java.io.File
import java.io.InputStream
import java.util.UUID
import kotlin.concurrent.thread

/**
 * Applies the user's [HoppyTheme] before any view is created, sets the system bar
 * icon style from the theme (not from the system night mode) and pauses/resumes the
 * decorative animations of the whole view tree with the activity lifecycle.
 */
abstract class ThemedActivity : AppCompatActivity() {
    private var appliedTheme: HoppyTheme? = null

    /** Theme this activity was created with. */
    protected val hoppyTheme: HoppyTheme get() = appliedTheme ?: wantedTheme()

    /** Theme to apply; the stored one unless a screen overrides it (first-run picker). */
    protected open fun wantedTheme(): HoppyTheme = Prefs.theme(this)

    override fun onCreate(savedInstanceState: Bundle?) {
        val theme = wantedTheme()
        appliedTheme = theme
        setTheme(theme.styleRes)
        // Bar icon colour must follow the app theme, not the system night mode
        // (default enableEdgeToEdge() would put dark icons on the dark Night theme).
        val bars = if (theme.isDark) {
            SystemBarStyle.dark(Color.TRANSPARENT)
        } else {
            SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        }
        enableEdgeToEdge(statusBarStyle = bars, navigationBarStyle = bars)
        super.onCreate(savedInstanceState)
    }

    override fun onStart() {
        super.onStart()
        HostAware.dispatch(window.decorView, true)
    }

    override fun onResume() {
        super.onResume()
        if (appliedTheme != null && appliedTheme != wantedTheme()) recreate()
    }

    override fun onStop() {
        HostAware.dispatch(window.decorView, false)
        super.onStop()
    }
}

object Format {
    fun size(bytes: Long): String {
        val units = arrayOf("B", "KB", "MB", "GB", "TB")
        var v = bytes.toDouble()
        var u = 0
        while (v >= 1024 && u < units.lastIndex) { v /= 1024; u++ }
        return if (u == 0) "$bytes B" else String.format("%.1f %s", v, units[u])
    }

    fun platform(ctx: Context, platform: String): String = when (platform) {
        "windows" -> "Windows"
        "android" -> "Android"
        "macos" -> "Mac"
        "ios" -> "iPhone"
        "linux" -> "Linux"
        else -> ctx.getString(R.string.platform_unknown)
    }
}

/**
 * A content URI to send. Unknown sizes are copied to cache first (the protocol needs sizes),
 * each into its own fresh directory under `cache/outgoing/`.
 */
class UriSource private constructor(
    private val ctx: Context,
    private val uri: Uri?,
    private val file: File?,
    override val name: String,
    override val size: Long,
) : Source {
    override fun open(): InputStream =
        file?.inputStream() ?: ctx.contentResolver.openInputStream(uri!!) ?: error("cannot open $uri")

    /** Deletes the cache copy, if any. Blocking. */
    fun cleanup() {
        file?.parentFile?.deleteRecursively()
    }

    companion object {
        private const val STALE_MS = 24 * 60 * 60 * 1000L

        /** Only other apps' `content:` URIs (see [SafeText.isReadableShare]). */
        fun isReadable(ctx: Context, uri: Uri): Boolean =
            SafeText.isReadableShare(uri.scheme, uri.authority, ctx.packageName)

        /** Blocking: call off the main thread. */
        fun resolve(ctx: Context, uri: Uri): UriSource {
            require(isReadable(ctx, uri)) { "refusing to read $uri" }
            var name: String? = null
            var size = -1L
            ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { c ->
                    if (c.moveToFirst()) {
                        name = c.getString(0)
                        if (!c.isNull(1)) size = c.getLong(1)
                    }
                }
            // DISPLAY_NAME comes from the other app: never use it as a path as-is.
            val displayName = SafeText.outgoingFileName(name ?: uri.lastPathSegment)
            if (size >= 0) return UriSource(ctx, uri, null, displayName, size)
            val copy = cacheFile(ctx, displayName)
            val input = ctx.contentResolver.openInputStream(uri) ?: error("cannot open $uri")
            input.use { copy.outputStream().use { out -> it.copyTo(out) } }
            return UriSource(ctx, null, copy, displayName, copy.length())
        }

        /** Shared text (no file) becomes a small .txt. Blocking. */
        fun fromText(ctx: Context, text: String): UriSource {
            val f = cacheFile(ctx, SafeText.outgoingFileName(ctx.getString(R.string.shared_text_file)))
            f.writeText(text)
            return UriSource(ctx, null, f, f.name, f.length())
        }

        /** `cache/outgoing/<random>/<name>`, checked to stay inside that directory. */
        private fun cacheFile(ctx: Context, name: String): File {
            val root = File(ctx.cacheDir, "outgoing").apply { mkdirs() }
            pruneStale(root)
            val dir = File(root, UUID.randomUUID().toString())
            check(dir.mkdirs()) { "cannot create $dir" }
            val file = File(dir, name)
            check(file.canonicalFile.parentFile == dir.canonicalFile) { "unsafe name" }
            return file
        }

        /** Copies left behind by a killed process. */
        private fun pruneStale(root: File) {
            val old = System.currentTimeMillis() - STALE_MS
            root.listFiles()?.filter { it.lastModified() < old }?.forEach { it.deleteRecursively() }
        }
    }
}

/** Loads the trust data of the device badges off the main thread (reads `trust.json`). */
fun refreshTrustBadges(activity: Activity, adapter: PeerAdapter) {
    val app = activity.applicationContext
    thread(name = "hoppy-trust-badges") {
        val ids = runCatching { Security.trustedIds(app) }.getOrDefault(emptyMap())
        activity.runOnUiThread { adapter.trusted = ids }
    }
}

class PeerAdapter(private val onClick: (Peer) -> Unit) :
    ListAdapter<Peer, PeerAdapter.Holder>(object : DiffUtil.ItemCallback<Peer>() {
        override fun areItemsTheSame(a: Peer, b: Peer) = a.device.id == b.device.id
        override fun areContentsTheSame(a: Peer, b: Peer) = a == b
    }) {

    var enabled = true
        set(value) {
            field = value
            notifyItemRangeChanged(0, itemCount)
        }

    /**
     * Remembered devices (id → verified), for the badges: 🔒 encrypted, ✓ verified,
     * "⚠ not encrypted" for old (v0) apps (docs/protocol.md §7.1). A remembered id always
     * gets 🔒: the session will be encrypted even if its announce lost the `crypto` field.
     */
    var trusted: Map<String, Boolean> = emptyMap()
        set(value) {
            if (field == value) return
            field = value
            notifyItemRangeChanged(0, itemCount)
        }

    /** Device shown emphasised (framed, "Selected") — a preselection waiting for a tap. */
    var highlightId: String? = null
        set(value) {
            if (field == value) return
            field = value
            notifyItemRangeChanged(0, itemCount)
        }

    class Holder(val b: ItemPeerBinding) : RecyclerView.ViewHolder(b.root) {
        val ripple: Drawable? = b.root.foreground
        val emphasised: Drawable by lazy { LayerDrawable(listOfNotNull(ripple, frame()).toTypedArray()) }

        private fun frame(): Drawable {
            val ctx = b.root.context
            val dm = ctx.resources.displayMetrics
            val corner = TypedValue().also { ctx.theme.resolveAttribute(R.attr.hoppyCorner, it, true) }
            return GradientDrawable().apply {
                cornerRadius = corner.getDimension(dm)
                setStroke(
                    (3 * dm.density).toInt(),
                    MaterialColors.getColor(b.root, androidx.appcompat.R.attr.colorPrimary),
                )
            }
        }
    }

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int) =
        Holder(ItemPeerBinding.inflate(LayoutInflater.from(parent.context), parent, false)).also {
            ThemeFonts.applyBodyBold(it.b.name)
            ThemeFonts.applyDisplay(it.b.initial)
        }

    override fun onBindViewHolder(h: Holder, position: Int) {
        val peer = getItem(position)
        val ctx = h.b.root.context
        val name = SafeText.label(peer.device.name)
        h.b.initial.text = name.take(1).uppercase()
        h.b.name.text = name
        val platform = Format.platform(ctx, peer.device.platform)
        val verified = trusted[peer.device.id]
        val (badge, spoken) = when {
            verified == true -> R.string.peer_secure_verified to R.string.peer_secure_verified_cd
            peer.encrypts || verified != null -> R.string.peer_secure to R.string.peer_secure_cd
            else -> R.string.peer_plain to R.string.peer_plain_cd
        }
        h.b.platform.text = ctx.getString(badge, platform)
        h.b.platform.contentDescription = ctx.getString(spoken, platform)
        val highlighted = peer.device.id == highlightId
        h.b.root.foreground = if (highlighted) h.emphasised else h.ripple
        ViewCompat.setStateDescription(h.b.root, if (highlighted) ctx.getString(R.string.peer_selected) else null)
        h.b.root.isEnabled = enabled
        h.b.root.alpha = if (enabled) 1f else 0.5f
        h.b.root.setOnClickListener { if (enabled) onClick(peer) }
    }
}
