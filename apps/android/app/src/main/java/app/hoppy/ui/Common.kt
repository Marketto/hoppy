package app.hoppy.ui

import android.content.Context
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.view.LayoutInflater
import android.view.ViewGroup
import androidx.activity.SystemBarStyle
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.recyclerview.widget.DiffUtil
import androidx.recyclerview.widget.ListAdapter
import androidx.recyclerview.widget.RecyclerView
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.Peer
import app.hoppy.core.Source
import app.hoppy.databinding.ItemPeerBinding
import app.hoppy.ui.theme.HoppyTheme
import app.hoppy.ui.theme.HostAware
import app.hoppy.ui.theme.ThemeFonts
import java.io.File
import java.io.InputStream

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

/** A content URI to send. Unknown sizes are copied to cache first (the protocol needs sizes). */
class UriSource private constructor(
    private val ctx: Context,
    private val uri: Uri?,
    private val file: File?,
    override val name: String,
    override val size: Long,
) : Source {
    override fun open(): InputStream =
        file?.inputStream() ?: ctx.contentResolver.openInputStream(uri!!) ?: error("cannot open $uri")

    companion object {
        /** Blocking: call off the main thread. */
        fun resolve(ctx: Context, uri: Uri): UriSource {
            var name: String? = null
            var size = -1L
            ctx.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { c ->
                    if (c.moveToFirst()) {
                        name = c.getString(0)
                        if (!c.isNull(1)) size = c.getLong(1)
                    }
                }
            val displayName = name ?: uri.lastPathSegment?.substringAfterLast('/') ?: "file"
            if (size >= 0) return UriSource(ctx, uri, null, displayName, size)
            val copy = File(ctx.cacheDir, "outgoing").apply { mkdirs() }.let { File(it, displayName) }
            ctx.contentResolver.openInputStream(uri)!!.use { input -> copy.outputStream().use { input.copyTo(it) } }
            return UriSource(ctx, null, copy, displayName, copy.length())
        }

        /** Shared text (no file) becomes a small .txt. */
        fun fromText(ctx: Context, text: String): UriSource {
            val f = File(File(ctx.cacheDir, "outgoing").apply { mkdirs() }, ctx.getString(R.string.shared_text_file))
            f.writeText(text)
            return UriSource(ctx, null, f, f.name, f.length())
        }
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

    class Holder(val b: ItemPeerBinding) : RecyclerView.ViewHolder(b.root)

    override fun onCreateViewHolder(parent: ViewGroup, viewType: Int) =
        Holder(ItemPeerBinding.inflate(LayoutInflater.from(parent.context), parent, false)).also {
            ThemeFonts.applyBodyBold(it.b.name)
            ThemeFonts.applyDisplay(it.b.initial)
        }

    override fun onBindViewHolder(h: Holder, position: Int) {
        val peer = getItem(position)
        val ctx = h.b.root.context
        h.b.initial.text = peer.device.name.trim().take(1).uppercase().ifEmpty { "?" }
        h.b.name.text = peer.device.name
        h.b.platform.text = Format.platform(ctx, peer.device.platform)
        h.b.root.isEnabled = enabled
        h.b.root.alpha = if (enabled) 1f else 0.5f
        h.b.root.setOnClickListener { if (enabled) onClick(peer) }
    }
}
