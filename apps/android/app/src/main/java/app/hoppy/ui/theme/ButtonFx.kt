package app.hoppy.ui.theme

import android.animation.Animator
import android.animation.Keyframe
import android.animation.ObjectAnimator
import android.animation.PropertyValuesHolder
import android.animation.ValueAnimator
import android.view.View
import android.view.animation.AccelerateDecelerateInterpolator
import android.view.animation.LinearInterpolator

/**
 * Small catalogue of button/decor motions used by [HoppyButton] and the home decor.
 * Every factory returns an un-started animator; callers start/cancel it with the
 * screen lifecycle and only when [Motion.enabled].
 */
object ButtonFx {
    private fun kf(f: Float, v: Float) = Keyframe.ofFloat(f, v)

    /** Purikura: a periodic jelly squish (then rest), repeating forever. */
    fun jelly(view: View, periodMs: Long = 3200L): ObjectAnimator {
        val sx = PropertyValuesHolder.ofKeyframe(
            View.SCALE_X,
            kf(0f, 1f), kf(0.06f, 1.06f), kf(0.12f, 0.95f), kf(0.18f, 1.03f), kf(0.24f, 0.99f), kf(0.30f, 1f), kf(1f, 1f),
        )
        val sy = PropertyValuesHolder.ofKeyframe(
            View.SCALE_Y,
            kf(0f, 1f), kf(0.06f, 0.92f), kf(0.12f, 1.05f), kf(0.18f, 0.97f), kf(0.24f, 1.01f), kf(0.30f, 1f), kf(1f, 1f),
        )
        return ObjectAnimator.ofPropertyValuesHolder(view, sx, sy).apply {
            duration = periodMs
            repeatCount = ValueAnimator.INFINITE
            interpolator = LinearInterpolator()
        }
    }

    /** Sticker "pop": a bounce with a little wobble every [periodMs]. */
    fun stickerPop(view: View, periodMs: Long = 2800L, baseRotation: Float = view.rotation): ObjectAnimator {
        val s = PropertyValuesHolder.ofKeyframe(
            View.SCALE_X,
            kf(0f, 1f), kf(0.08f, 1.18f), kf(0.16f, 0.94f), kf(0.24f, 1.04f), kf(0.32f, 1f), kf(1f, 1f),
        )
        val s2 = PropertyValuesHolder.ofKeyframe(
            View.SCALE_Y,
            kf(0f, 1f), kf(0.08f, 1.18f), kf(0.16f, 0.94f), kf(0.24f, 1.04f), kf(0.32f, 1f), kf(1f, 1f),
        )
        val r = PropertyValuesHolder.ofKeyframe(
            View.ROTATION,
            kf(0f, baseRotation), kf(0.10f, baseRotation + 8f), kf(0.20f, baseRotation - 5f),
            kf(0.30f, baseRotation), kf(1f, baseRotation),
        )
        return ObjectAnimator.ofPropertyValuesHolder(view, s, s2, r).apply {
            duration = periodMs
            repeatCount = ValueAnimator.INFINITE
            interpolator = LinearInterpolator()
        }
    }

    /** Manga: quick horizontal shake (one shot). */
    fun shake(view: View, amplitudePx: Float): ObjectAnimator =
        ObjectAnimator.ofPropertyValuesHolder(
            view,
            PropertyValuesHolder.ofKeyframe(
                View.TRANSLATION_X,
                kf(0f, 0f), kf(0.15f, -amplitudePx), kf(0.35f, amplitudePx), kf(0.55f, -amplitudePx * 0.6f),
                kf(0.75f, amplitudePx * 0.4f), kf(1f, 0f),
            ),
        ).apply { duration = 320L }

    /** Gentle twinkle: alpha + scale breathing (Yume stars, decor). */
    fun twinkle(view: View, periodMs: Long = 1800L, startDelayMs: Long = 0L): ObjectAnimator =
        ObjectAnimator.ofPropertyValuesHolder(
            view,
            PropertyValuesHolder.ofFloat(View.ALPHA, 1f, 0.35f),
            PropertyValuesHolder.ofFloat(View.SCALE_X, 1f, 0.75f),
            PropertyValuesHolder.ofFloat(View.SCALE_Y, 1f, 0.75f),
        ).apply {
            duration = periodMs / 2
            startDelay = startDelayMs
            repeatCount = ValueAnimator.INFINITE
            repeatMode = ValueAnimator.REVERSE
            interpolator = AccelerateDecelerateInterpolator()
        }

    /** One-shot pop-in (e.g. "とどいた！"). */
    fun popIn(view: View): Animator {
        view.scaleX = 0.3f
        view.scaleY = 0.3f
        view.alpha = 0f
        return ObjectAnimator.ofPropertyValuesHolder(
            view,
            PropertyValuesHolder.ofKeyframe(View.SCALE_X, kf(0f, 0.3f), kf(0.6f, 1.15f), kf(1f, 1f)),
            PropertyValuesHolder.ofKeyframe(View.SCALE_Y, kf(0f, 0.3f), kf(0.6f, 1.15f), kf(1f, 1f)),
            PropertyValuesHolder.ofFloat(View.ALPHA, 0f, 1f),
        ).apply { duration = 420L }
    }
}
