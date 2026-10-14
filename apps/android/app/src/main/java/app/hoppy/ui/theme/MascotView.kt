package app.hoppy.ui.theme

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.Keyframe
import android.animation.ObjectAnimator
import android.animation.PropertyValuesHolder
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.drawable.Animatable
import android.util.AttributeSet
import android.view.View
import android.view.animation.LinearInterpolator
import androidx.appcompat.widget.AppCompatImageView
import app.hoppy.R

/**
 * Hoppy himself: the theme's AnimatedVectorDrawable (ears, blink, blush, glint) plus
 * whole-body motion per theme (hop / bob / sway / nod). [celebrate] swaps to the happy
 * mascot and does a few big hops. Everything stops when hidden, detached, after
 * Activity.onStop ([HostAware]) and when system animations are off.
 */
class MascotView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : AppCompatImageView(context, attrs), HostAware {
    val hoppyTheme: HoppyTheme = HoppyTheme.of(context)
    private var hostStarted = true
    private var idle: ObjectAnimator? = null
    private var celebration: Animator? = null
    private var celebrating = false
    private val restore = Runnable { endCelebration() }

    init {
        setImageResource(hoppyTheme.mascotAnim)
        if (contentDescription == null) contentDescription = context.getString(R.string.mascot_description)
    }

    override fun setHostStarted(started: Boolean) {
        hostStarted = started
        sync()
    }

    private fun sync() {
        val run = hostStarted && shouldAnimate()
        val avd = drawable as? Animatable
        if (run) {
            if (avd != null && !avd.isRunning) avd.start()
            if (idle == null && !celebrating && width > 0) startIdle()
        } else {
            avd?.stop()
            stopIdle()
            celebration?.cancel()
        }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        sync()
    }

    override fun onDetachedFromWindow() {
        removeCallbacks(restore)
        (drawable as? Animatable)?.stop()
        stopIdle()
        celebration?.cancel()
        super.onDetachedFromWindow()
    }

    override fun onWindowVisibilityChanged(visibility: Int) {
        super.onWindowVisibilityChanged(visibility)
        sync()
    }

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        post { sync() }
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        pivotX = w / 2f
        pivotY = h * 0.92f // feet
        post { sync() }
    }

    // --- idle body motion --------------------------------------------------------------

    private fun startIdle() {
        idle = when (hoppyTheme) {
            HoppyTheme.PURIKURA -> hop(durationMs = 2200L, height = dp(18f), squash = 0.12f)
            HoppyTheme.MANGA -> hop(durationMs = 1600L, height = dp(28f), squash = 0.16f)
            HoppyTheme.SHOWA -> ObjectAnimator.ofFloat(this, View.TRANSLATION_Y, 0f, -dp(5f)).apply {
                duration = 1400L
                repeatMode = ValueAnimator.REVERSE
            }
            HoppyTheme.YUME -> ObjectAnimator.ofFloat(this, View.ROTATION, -4f, 4f).apply {
                duration = 1800L
                repeatMode = ValueAnimator.REVERSE
            }
            HoppyTheme.NIGHT -> ObjectAnimator.ofPropertyValuesHolder(
                this,
                PropertyValuesHolder.ofKeyframe(
                    View.ROTATION,
                    Keyframe.ofFloat(0f, 0f), Keyframe.ofFloat(0.12f, 3f), Keyframe.ofFloat(0.24f, 0f),
                    Keyframe.ofFloat(0.36f, 2f), Keyframe.ofFloat(0.48f, 0f), Keyframe.ofFloat(1f, 0f),
                ),
            ).apply { duration = 4200L }
        }.apply {
            repeatCount = ValueAnimator.INFINITE
            start()
        }
    }

    private fun stopIdle() {
        idle?.cancel()
        idle = null
        translationY = 0f
        scaleX = 1f
        scaleY = 1f
        rotation = 0f
    }

    /** One hop with squash and stretch, then rest until the end of [durationMs]. */
    private fun hop(durationMs: Long, height: Float, squash: Float): ObjectAnimator {
        fun kf(f: Float, v: Float) = Keyframe.ofFloat(f, v)
        val up = -height
        val ty = PropertyValuesHolder.ofKeyframe(
            View.TRANSLATION_Y,
            kf(0f, 0f), kf(0.10f, 0f), kf(0.22f, up * 0.75f), kf(0.30f, up), kf(0.40f, up * 0.6f),
            kf(0.48f, 0f), kf(0.56f, 0f), kf(1f, 0f),
        )
        val sy = PropertyValuesHolder.ofKeyframe(
            View.SCALE_Y,
            kf(0f, 1f), kf(0.10f, 1f - squash), kf(0.20f, 1f + squash * 0.7f), kf(0.30f, 1f),
            kf(0.44f, 1f + squash * 0.4f), kf(0.50f, 1f - squash * 0.8f), kf(0.60f, 1f), kf(1f, 1f),
        )
        val sx = PropertyValuesHolder.ofKeyframe(
            View.SCALE_X,
            kf(0f, 1f), kf(0.10f, 1f + squash), kf(0.20f, 1f - squash * 0.5f), kf(0.30f, 1f),
            kf(0.44f, 1f - squash * 0.3f), kf(0.50f, 1f + squash * 0.8f), kf(0.60f, 1f), kf(1f, 1f),
        )
        return ObjectAnimator.ofPropertyValuesHolder(this, ty, sy, sx).apply {
            duration = durationMs
            interpolator = LinearInterpolator()
        }
    }

    // --- celebration -------------------------------------------------------------------

    /** Delivered! Happy face and a few big hops, then back to idle. */
    fun celebrate() {
        celebrating = true
        stopIdle()
        celebration?.cancel() // its end listener posts [restore]: remove it afterwards
        removeCallbacks(restore)
        setImageResource(hoppyTheme.happyAnim)
        val run = hostStarted && shouldAnimate()
        if (run) (drawable as? Animatable)?.start()
        if (!run) {
            postDelayed(restore, 1600L)
            return
        }
        celebration = hop(durationMs = 620L, height = dp(34f), squash = 0.18f).apply {
            repeatCount = 2
            addListener(object : AnimatorListenerAdapter() {
                override fun onAnimationEnd(animation: Animator) {
                    celebration = null
                    postDelayed(restore, 500L)
                }
            })
            start()
        }
    }

    private fun endCelebration() {
        if (!celebrating) return
        celebrating = false
        (drawable as? Animatable)?.stop()
        setImageResource(hoppyTheme.mascotAnim)
        sync()
    }
}
