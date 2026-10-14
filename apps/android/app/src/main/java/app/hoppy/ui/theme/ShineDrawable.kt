package app.hoppy.ui.theme

import android.animation.Animator
import android.animation.AnimatorListenerAdapter
import android.animation.ValueAnimator
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.ColorFilter
import android.graphics.LinearGradient
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.drawable.Animatable
import android.graphics.drawable.Drawable
import android.view.animation.AccelerateDecelerateInterpolator

/**
 * A glossy diagonal light band that sweeps across the bounds, then pauses.
 * Meant as a View foreground (`view.foreground = shine`) on primary buttons.
 * The band is clipped to a rounded rect ([insets] + [cornerRadius]) so it never
 * leaks outside the button face. One shader, moved with a matrix: no per-frame allocation.
 */
class ShineDrawable(
    private val sweepMs: Long = 900L,
    private val pauseMs: Long = 1500L,
    /** Peak alpha of the band, 0..1. */
    private val strength: Float = 0.45f,
) : Drawable(), Animatable {
    /** Distance from the drawable bounds to the face that the shine is clipped to. */
    val insets = RectF()
    /** Corner radius of the face; use a large value for pills. */
    var cornerRadius = 0f
        set(value) { field = value; rebuildClip() }

    private var progress = -1f // <0: idle (nothing drawn)
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val clip = Path()
    private val face = RectF()
    private val shaderMatrix = Matrix()
    private var bandWidth = 0f
    private var animator: ValueAnimator? = null

    private fun rebuildShader() {
        val b = bounds
        if (b.isEmpty) return
        bandWidth = (b.height() * 1.2f).coerceAtLeast(24f)
        val peak = Color.argb((255 * strength).toInt().coerceIn(0, 255), 255, 255, 255)
        paint.shader = LinearGradient(
            0f, 0f, bandWidth, 0f,
            intArrayOf(Color.TRANSPARENT, peak, Color.TRANSPARENT), floatArrayOf(0f, 0.5f, 1f),
            Shader.TileMode.CLAMP,
        )
    }

    private fun rebuildClip() {
        val b = bounds
        face.set(b.left + insets.left, b.top + insets.top, b.right - insets.right, b.bottom - insets.bottom)
        clip.reset()
        if (!face.isEmpty) {
            val r = cornerRadius.coerceAtMost(face.height() / 2f)
            clip.addRoundRect(face, r, r, Path.Direction.CW)
        }
    }

    override fun onBoundsChange(bounds: android.graphics.Rect) {
        super.onBoundsChange(bounds)
        rebuildShader()
        rebuildClip()
    }

    /** Call after changing [insets]. */
    fun invalidateGeometry() {
        rebuildClip()
        invalidateSelf()
    }

    override fun draw(canvas: Canvas) {
        if (progress < 0f || clip.isEmpty || paint.shader == null) return
        val travel = face.width() + bandWidth * 2f
        val x = face.left - bandWidth + travel * progress
        shaderMatrix.reset()
        shaderMatrix.setRotate(-20f, 0f, 0f)
        shaderMatrix.postTranslate(x, 0f)
        paint.shader.setLocalMatrix(shaderMatrix)
        val save = canvas.save()
        canvas.clipPath(clip)
        canvas.drawRect(face, paint)
        canvas.restoreToCount(save)
    }

    override fun start() {
        if (animator != null || !Motion.enabled()) return
        val cycle = sweepMs + pauseMs
        val sweepFraction = sweepMs.toFloat() / cycle
        animator = ValueAnimator.ofFloat(0f, 1f).apply {
            duration = cycle
            repeatCount = ValueAnimator.INFINITE
            interpolator = null // linear; easing is applied below to the sweep part only
            addUpdateListener {
                val f = it.animatedFraction
                progress = if (f <= sweepFraction) EASE.getInterpolation(f / sweepFraction) else -1f
                invalidateSelf()
            }
            addListener(object : AnimatorListenerAdapter() {
                override fun onAnimationCancel(animation: Animator) {
                    progress = -1f
                    invalidateSelf()
                }
            })
            start()
        }
    }

    override fun stop() {
        animator?.cancel()
        animator = null
        progress = -1f
        invalidateSelf()
    }

    override fun isRunning(): Boolean = animator != null

    override fun setAlpha(alpha: Int) {
        paint.alpha = alpha
    }

    override fun setColorFilter(colorFilter: ColorFilter?) {
        paint.colorFilter = colorFilter
    }

    @Deprecated("Deprecated in Java")
    override fun getOpacity(): Int = PixelFormat.TRANSLUCENT

    private companion object {
        val EASE = AccelerateDecelerateInterpolator()
    }
}
