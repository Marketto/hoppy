package app.hoppy.audio

import android.app.Activity
import android.app.Application
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaPlayer
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.annotation.RawRes
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.service.ReceiveService
import app.hoppy.ui.theme.HoppyTheme

/**
 * Theme music (docs/design/themes.md "Music"). Main thread only.
 *
 * Plays the theme's `idle` loop while any Hoppy activity is started, switches to the
 * `transfer` loop on the next bar boundary while this device **sends**, plays `done`
 * once on delivery, and is silent while this device **receives** (the receiver never
 * plays music), when muted (overflow menu), when another app is already playing, and
 * after any audio-focus loss. Players are released after [RELEASE_AFTER_MS] of silence
 * or on memory trim. Reduced motion does not mute.
 */
object Music {
    private const val VOLUME = 0.6f
    private const val FADE_IN_MS = 200L
    private const val FADE_OUT_MS = 200L
    private const val CROSSFADE_MS = 300L
    private const val FADE_STEP_MS = 20L
    /** Activity A stops just after B starts; rotation stops then starts: don't fade for that. */
    private const val STOP_GRACE_MS = 250L
    private const val RELEASE_AFTER_MS = 30_000L

    private lateinit var app: Application
    private lateinit var audio: AudioManager
    private val main = Handler(Looper.getMainLooper())
    private val attributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_GAME)
        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
        .build()

    private var started = 0
    private var visible = false
    private var muted = false
    /** Focus lost for good (another player took over): silent until the next visit. */
    private var focusLost = false
    /** Focus lost for a while (call, notification): we keep our request and wait for GAIN. */
    private var focusPaused = false
    private var focus: AudioFocusRequest? = null
    /** Sends in progress on this device (normally 0 or 1). */
    private var transfers = 0
    /** Theme being previewed in the theme picker; wins over the stored one. */
    private var preview: HoppyTheme? = null

    /** The audible (or paused, resumable) loop. */
    private var loop: Track? = null
    /** Next loop, preparing or waiting for the bar boundary. */
    private var pending: Track? = null
    private var pendingStart: Runnable? = null
    private var jingle: Track? = null

    private val goneInvisible = Runnable { visible = false; reconcile() }
    private val releaseIdle = Runnable { releaseSilent() }

    fun init(application: Application) {
        app = application
        audio = application.getSystemService(AudioManager::class.java)
        muted = !Prefs.music(application)
        application.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) {
                if (started++ == 0) focusLost = false // a fresh visit may try again
                main.removeCallbacks(goneInvisible)
                visible = true
                reconcile()
            }

            override fun onActivityStopped(activity: Activity) {
                if (--started == 0) main.postDelayed(goneInvisible, STOP_GRACE_MS)
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) = Unit
            override fun onActivityResumed(activity: Activity) = Unit
            override fun onActivityPaused(activity: Activity) = Unit
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) = Unit
            override fun onActivityDestroyed(activity: Activity) = Unit
        })
        // Called on service threads.
        ReceiveService.addStateListener { main.post { reconcile() } }
    }

    fun enabled(ctx: Context): Boolean = Prefs.music(ctx)

    fun setEnabled(ctx: Context, value: Boolean) {
        Prefs.setMusic(ctx, value)
        muted = !value
        reconcile()
    }

    /** Theme picker: let the user hear the card they are on (null = back to the stored theme). */
    fun preview(theme: HoppyTheme?) {
        preview = theme
        reconcile()
    }

    /** A send from this device started: idle → transfer on the next bar. */
    fun transferStarted() {
        transfers++
        reconcile()
    }

    /** The send ended: [delivered] plays the jingle, otherwise back to idle. */
    fun transferEnded(delivered: Boolean) {
        transfers = (transfers - 1).coerceAtLeast(0)
        if (delivered && transfers == 0 && audible()) playJingle()
        reconcile()
    }

    /** From `Application.onTrimMemory`: drop players that are not playing. */
    fun trimMemory() {
        if (::app.isInitialized && !audible()) releaseSilent()
    }

    private fun audible() = !muted && !focusLost && !focusPaused && !ReceiveService.receiving

    private fun theme(): HoppyTheme = preview ?: Prefs.theme(app)

    private fun wantedKind() = if (transfers > 0) MusicTiming.TRANSFER else MusicTiming.IDLE

    /** Brings the players in line with the state above. */
    private fun reconcile() {
        if (!::app.isInitialized) return
        if (!audible()) {
            // The jingle may finish after the screen closes (delivered → finish), never over others.
            jingle?.let { j -> jingle = null; fade(j, 0f, FADE_OUT_MS) { j.release() } }
        }
        if (!audible() || !visible) {
            cancelPending()
            loop?.let { l -> if (l.player.isPlaying) fade(l, 0f, FADE_OUT_MS) { pauseIfSilent(l) } }
            if (jingle == null && !focusPaused) abandonFocus()
            main.removeCallbacks(releaseIdle)
            main.postDelayed(releaseIdle, RELEASE_AFTER_MS)
            return
        }
        main.removeCallbacks(releaseIdle)
        if (jingle != null) {
            // The jingle plays alone; the loop comes back when it ends.
            cancelPending()
            loop?.let { l -> if (l.player.isPlaying) fade(l, 0f, FADE_OUT_MS) { pauseIfSilent(l) } }
            return
        }
        val theme = theme()
        val kind = wantedKind()
        val res = resource(theme, kind)
        val cur = loop
        if (cur != null && cur.res == res) {
            cancelPending()
            if (!cur.player.isPlaying) {
                if (!requestFocus()) return
                cur.player.start()
            }
            fade(cur, 1f, FADE_IN_MS)
            return
        }
        if (pending?.res == res) return
        cancelPending()
        if (cur == null || !cur.player.isPlaying) {
            // Nothing audible: start the new loop right away.
            cur?.release()
            loop = null
            if (!requestFocus()) return
            val next = Track.create(app, res, theme, kind, looping = true) ?: return
            pending = next
            next.whenPrepared {
                if (pending !== next) return@whenPrepared
                pending = null
                loop = next
                next.player.start()
                fade(next, 1f, FADE_IN_MS)
            }
            return
        }
        // Crossfade from the playing loop; a change of rhythm (idle → transfer) lands on a bar.
        val alignToBar = cur.theme == theme && cur.kind == MusicTiming.IDLE && kind == MusicTiming.TRANSFER
        val next = Track.create(app, res, theme, kind, looping = true) ?: return
        pending = next
        next.whenPrepared {
            if (pending !== next) return@whenPrepared
            val delay = if (alignToBar && !cur.released) {
                val bar = MusicTiming.barMs(cur.theme.name.lowercase(), cur.kind) ?: 0.0
                MusicTiming.delayToNextBar(cur.player.currentPosition.toLong(), bar)
            } else {
                0L
            }
            val start = Runnable {
                if (pending !== next) return@Runnable
                pending = null
                pendingStart = null
                val old = loop
                loop = next
                next.player.start()
                fade(next, 1f, CROSSFADE_MS)
                if (old != null) fade(old, 0f, CROSSFADE_MS) { old.release() }
            }
            pendingStart = start
            main.postDelayed(start, delay)
        }
    }

    private fun playJingle() {
        if (!visible || !requestFocus()) return
        cancelPending()
        val t = theme()
        val j = Track.create(app, resource(t, MusicTiming.DONE), t, MusicTiming.DONE, looping = false) ?: return
        jingle = j
        j.player.setOnCompletionListener {
            if (jingle === j) jingle = null
            j.release()
            // The finished transfer loop is not resumed: back to idle (or silence).
            loop?.let { l -> if (!l.player.isPlaying) { l.release(); loop = null } }
            reconcile()
        }
        j.whenPrepared {
            if (jingle !== j) return@whenPrepared
            j.setGain(1f)
            j.player.start()
        }
    }

    private fun pauseIfSilent(t: Track) {
        if (!t.released && t.gain == 0f && t.player.isPlaying) t.player.pause()
    }

    private fun cancelPending() {
        pendingStart?.let { main.removeCallbacks(it) }
        pendingStart = null
        pending?.release()
        pending = null
    }

    private fun releaseSilent() {
        loop?.let { if (it.released || !it.player.isPlaying) { it.release(); loop = null } }
        jingle?.let { if (it.released || !it.player.isPlaying) { it.release(); jingle = null } }
        if (!visible) cancelPending()
    }

    // ---- audio focus ----------------------------------------------------------------

    private val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
        when (change) {
            AudioManager.AUDIOFOCUS_GAIN -> { focusPaused = false; reconcile() }
            AudioManager.AUDIOFOCUS_LOSS -> { focusLost = true; focusPaused = false; abandonFocus(); reconcile() }
            else -> if (change < 0) { focusPaused = true; reconcile() } // transient / can-duck: pause
        }
    }

    /** Never starts over the user's own audio: someone else playing → stay silent. */
    private fun requestFocus(): Boolean {
        if (focus != null) return true
        // isMusicActive also counts our own loop still fading out.
        val ours = loop?.let { !it.released && it.player.isPlaying } == true
        if (!ours && audio.isMusicActive) return false
        val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_MAY_DUCK)
            .setAudioAttributes(attributes)
            .setWillPauseWhenDucked(true)
            .setOnAudioFocusChangeListener(focusListener, main)
            .build()
        if (audio.requestAudioFocus(req) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) return false
        focus = req
        return true
    }

    private fun abandonFocus() {
        focus?.let { audio.abandonAudioFocusRequest(it) }
        focus = null
    }

    // ---- players --------------------------------------------------------------------

    /** Fades [t] to [target] (× [VOLUME]) over [ms]; replaces any fade running on it. */
    private fun fade(t: Track, target: Float, ms: Long, then: (() -> Unit)? = null) {
        t.cancelFade()
        val from = t.gain
        if (from == target || ms <= 0) {
            t.setGain(target)
            then?.invoke()
            return
        }
        val begin = SystemClock.uptimeMillis()
        val step = object : Runnable {
            override fun run() {
                if (t.released) return
                val p = (SystemClock.uptimeMillis() - begin).toFloat() / ms
                t.setGain(if (p >= 1f) target else MusicTiming.fadeGain(from, target, p))
                if (p >= 1f) {
                    t.fadeStep = null
                    then?.invoke()
                } else {
                    main.postDelayed(this, FADE_STEP_MS)
                }
            }
        }
        t.fadeStep = step
        main.post(step)
    }

    @RawRes
    private fun resource(theme: HoppyTheme, kind: String): Int = when (theme) {
        HoppyTheme.PURIKURA -> pick(kind, R.raw.music_purikura_idle, R.raw.music_purikura_transfer, R.raw.music_purikura_done)
        HoppyTheme.SHOWA -> pick(kind, R.raw.music_showa_idle, R.raw.music_showa_transfer, R.raw.music_showa_done)
        HoppyTheme.MANGA -> pick(kind, R.raw.music_manga_idle, R.raw.music_manga_transfer, R.raw.music_manga_done)
        HoppyTheme.YUME -> pick(kind, R.raw.music_yume_idle, R.raw.music_yume_transfer, R.raw.music_yume_done)
        HoppyTheme.NIGHT -> pick(kind, R.raw.music_night_idle, R.raw.music_night_transfer, R.raw.music_night_done)
    }

    private fun pick(kind: String, idle: Int, transfer: Int, done: Int) = when (kind) {
        MusicTiming.TRANSFER -> transfer
        MusicTiming.DONE -> done
        else -> idle
    }

    private class Track(
        @RawRes val res: Int,
        val theme: HoppyTheme,
        val kind: String,
        val player: MediaPlayer,
    ) {
        var gain = 0f
            private set
        var released = false
            private set
        var fadeStep: Runnable? = null
        private var prepared = false
        private var onPrepared: (() -> Unit)? = null

        fun setGain(g: Float) {
            gain = g
            if (!released) player.setVolume(g * VOLUME, g * VOLUME)
        }

        fun whenPrepared(block: () -> Unit) {
            if (prepared) block() else onPrepared = block
        }

        fun cancelFade() {
            fadeStep?.let { main.removeCallbacks(it) }
            fadeStep = null
        }

        fun release() {
            if (released) return
            cancelFade()
            released = true
            onPrepared = null
            runCatching { player.release() }
        }

        companion object {
            fun create(ctx: Context, @RawRes res: Int, theme: HoppyTheme, kind: String, looping: Boolean): Track? {
                val player = MediaPlayer()
                val track = Track(res, theme, kind, player)
                return runCatching {
                    player.setAudioAttributes(attributes)
                    ctx.resources.openRawResourceFd(res).use {
                        player.setDataSource(it.fileDescriptor, it.startOffset, it.length)
                    }
                    player.isLooping = looping
                    player.setVolume(0f, 0f)
                    // Callbacks arrive on the main looper (the player is created there).
                    player.setOnPreparedListener {
                        track.prepared = true
                        track.onPrepared?.also { track.onPrepared = null }?.invoke()
                    }
                    player.setOnErrorListener { _, _, _ ->
                        track.release()
                        if (loop === track) loop = null
                        if (pending === track) pending = null
                        if (jingle === track) jingle = null
                        true
                    }
                    player.prepareAsync()
                    track
                }.getOrElse {
                    track.release()
                    null
                }
            }
        }
    }
}
