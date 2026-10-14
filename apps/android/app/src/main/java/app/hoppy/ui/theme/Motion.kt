package app.hoppy.ui.theme

import android.animation.TimeAnimator
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Typeface
import android.os.SystemClock
import android.util.TypedValue
import android.view.View
import android.view.ViewGroup
import android.widget.TextView
import androidx.annotation.AttrRes
import androidx.core.content.res.ResourcesCompat
import app.hoppy.R

/** Global motion switch: honours the system "Remove animations" setting. */
object Motion {
    fun enabled(): Boolean = ValueAnimator.areAnimatorsEnabled()
}

fun View.dp(v: Float): Float = v * resources.displayMetrics.density
fun Context.dp(v: Float): Float = v * resources.displayMetrics.density

/**
 * True when a decorative animation on this view may run: attached, in a visible
 * window (false after Activity.onStop), shown, and system animations enabled.
 */
fun View.shouldAnimate(): Boolean =
    isAttachedToWindow && windowVisibility == View.VISIBLE && isShown && Motion.enabled()

/**
 * Per-view frame clock for Canvas animations: a TimeAnimator that invalidates the view
 * at most ~[fps] times per second. When stopped, [now] is frozen so the view draws a
 * calm static frame. Time is uptime-based so phases continue smoothly after a restart.
 */
class FrameTicker(private val view: View, fps: Int = 30) {
    private val minFrameMs = 1000L / fps
    private var lastFrame = 0L
    private var frozenAt = 0L
    private var animator: TimeAnimator? = null

    val running: Boolean get() = animator != null

    /** Milliseconds for drawing. Constant while stopped. */
    fun now(): Long = if (running) SystemClock.uptimeMillis() else frozenAt

    fun update(shouldRun: Boolean) {
        if (shouldRun == running) return
        if (shouldRun) {
            animator = TimeAnimator().apply {
                setTimeListener { _, _, _ ->
                    val t = SystemClock.uptimeMillis()
                    if (t - lastFrame >= minFrameMs) {
                        lastFrame = t
                        view.invalidate()
                    }
                }
                start()
            }
        } else {
            animator?.cancel()
            animator = null
            view.invalidate()
        }
    }
}

/**
 * Views with decorative animations. Activities call [HostAware.dispatch] from
 * onStart/onStop so everything stops while the screen is not visible (the views also
 * stop on detach, window hidden and when system animations are off).
 */
interface HostAware {
    fun setHostStarted(started: Boolean)

    companion object {
        fun dispatch(root: View, started: Boolean) {
            if (root is HostAware) root.setHostStarted(started)
            if (root is ViewGroup) {
                for (i in 0 until root.childCount) dispatch(root.getChildAt(i), started)
            }
        }
    }
}

/** Theme fonts resolved from the theme attributes (see res/values/attrs.xml). */
object ThemeFonts {
    fun get(ctx: Context, @AttrRes attr: Int): Typeface? {
        val tv = TypedValue()
        if (!ctx.theme.resolveAttribute(attr, tv, true) || tv.resourceId == 0) return null
        return runCatching { ResourcesCompat.getFont(ctx, tv.resourceId) }.getOrNull()
    }

    fun display(ctx: Context): Typeface? = get(ctx, R.attr.hoppyDisplayFont)
    fun bodyBold(ctx: Context): Typeface? = get(ctx, R.attr.hoppyBodyBoldFont)

    fun applyDisplay(vararg views: TextView) {
        views.forEach { v -> display(v.context)?.let { v.typeface = it } }
    }

    fun applyBodyBold(vararg views: TextView) {
        views.forEach { v -> bodyBold(v.context)?.let { v.typeface = it } }
    }
}
