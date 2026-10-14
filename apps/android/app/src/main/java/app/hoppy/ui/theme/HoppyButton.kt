package app.hoppy.ui.theme

import android.animation.ObjectAnimator
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Shader
import android.util.AttributeSet
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.widget.Button
import androidx.appcompat.widget.AppCompatTextView
import androidx.core.content.ContextCompat
import androidx.core.graphics.ColorUtils
import app.hoppy.R

/**
 * The single primary action of a screen, drawn per theme (docs/design/themes.md):
 * Purikura gradient pill with white border, ring and lavender drop + jelly + shine;
 * Showa vermilion slab with ink border and hard shadow + prominent shine + press-in;
 * Manga ink slab with pink hard shadow + press-in + shake of [shakeTarget];
 * Yume gradient pill with glossy top + glow pulse + shine; Night teal + faint glint.
 * A TextView (not MaterialButton) so the face can be fully custom; it reports itself
 * to accessibility services as a Button.
 */
class HoppyButton @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : AppCompatTextView(context, attrs), HostAware {
    val hoppyTheme: HoppyTheme = HoppyTheme.of(context)

    /** Optional decor label shaken on press (Manga "ピョン!"). */
    var shakeTarget: View? = null

    private var hostStarted = true
    private val shine: ShineDrawable = when (hoppyTheme) {
        HoppyTheme.SHOWA -> ShineDrawable(sweepMs = 800L, pauseMs = 1600L, strength = 0.6f)
        HoppyTheme.NIGHT -> ShineDrawable(sweepMs = 1400L, pauseMs = 4200L, strength = 0.16f)
        HoppyTheme.MANGA -> ShineDrawable(sweepMs = 700L, pauseMs = 3000L, strength = 0.0f)
        else -> ShineDrawable(sweepMs = 900L, pauseMs = 1700L, strength = 0.45f)
    }
    private val hasShine = hoppyTheme != HoppyTheme.MANGA
    private var loop: ObjectAnimator? = null
    private var glow = 0f

    private val facePaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val face = RectF()
    private val tmp = RectF()
    private val gloss = RectF()
    private var faceShader: Shader? = null

    // Geometry per theme, in px.
    private val shadow = when (hoppyTheme) {
        HoppyTheme.SHOWA, HoppyTheme.MANGA -> dp(6f)
        HoppyTheme.PURIKURA -> dp(5f)
        else -> 0f
    }
    private val margin = when (hoppyTheme) {
        HoppyTheme.YUME -> dp(8f)
        else -> 0f
    }
    private val faceStroke = when (hoppyTheme) {
        HoppyTheme.PURIKURA -> dp(7f) // 2dp ring + 5dp white border
        HoppyTheme.SHOWA -> dp(3f)
        else -> 0f
    }

    private fun color(id: Int) = ContextCompat.getColor(context, id)

    init {
        gravity = Gravity.CENTER
        // Clickable/focusable come from setOnClickListener (focusable="auto"), so the
        // non-interactive sample on the theme picker stays inert.
        minHeight = dp(56f).toInt()
        maxLines = 1
        ThemeFonts.display(context)?.let { typeface = it }
        setTextSize(TypedValue.COMPLEX_UNIT_SP, if (hoppyTheme == HoppyTheme.NIGHT) 18f else 20f)
        setAllCaps(hoppyTheme == HoppyTheme.SHOWA)
        if (hoppyTheme == HoppyTheme.SHOWA) letterSpacing = 0.08f
        setTextColor(
            when (hoppyTheme) {
                HoppyTheme.PURIKURA, HoppyTheme.YUME, HoppyTheme.MANGA -> Color.WHITE
                HoppyTheme.SHOWA -> color(R.color.showa_card)
                HoppyTheme.NIGHT -> color(R.color.night_on_teal)
            },
        )
        // Legibility of white on pastel faces (large bold text).
        when (hoppyTheme) {
            HoppyTheme.PURIKURA -> setShadowLayer(dp(2f), 0f, dp(1f), color(R.color.purikura_primary_dark))
            HoppyTheme.YUME -> setShadowLayer(dp(2f), 0f, dp(1f), color(R.color.yume_text))
            else -> Unit
        }
        background = null // the face is drawn in onDraw; set before padding
        val side = dp(20f).toInt()
        setPadding(
            (margin + faceStroke).toInt() + side,
            (margin + faceStroke).toInt(),
            (margin + faceStroke + shadowX()).toInt() + side,
            (margin + faceStroke + shadow).toInt(),
        )
        if (hasShine) foreground = shine
    }

    private fun shadowX() = if (hoppyTheme == HoppyTheme.PURIKURA) 0f else shadow

    override fun getAccessibilityClassName(): CharSequence = Button::class.java.name

    // --- lifecycle ---------------------------------------------------------------------

    override fun setHostStarted(started: Boolean) {
        hostStarted = started
        sync()
    }

    private fun sync() {
        val run = hostStarted && shouldAnimate()
        if (run) {
            if (hasShine) shine.start()
            if (loop == null) loop = when (hoppyTheme) {
                HoppyTheme.PURIKURA -> ButtonFx.jelly(this).also { it.start() }
                HoppyTheme.YUME -> ObjectAnimator.ofFloat(this, GLOW, 0f, 1f).apply {
                    duration = 1600L
                    repeatCount = ValueAnimator.INFINITE
                    repeatMode = ValueAnimator.REVERSE
                    start()
                }
                else -> null
            }
        } else {
            shine.stop()
            loop?.cancel()
            loop = null
            scaleX = 1f
            scaleY = 1f
            glow = 0f
            invalidate()
        }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        sync()
    }

    override fun onDetachedFromWindow() {
        shine.stop()
        loop?.cancel()
        loop = null
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

    override fun setEnabled(enabled: Boolean) {
        super.setEnabled(enabled)
        alpha = if (enabled) 1f else 0.5f
    }

    // --- press -------------------------------------------------------------------------

    private val pressesIntoShadow get() = hoppyTheme == HoppyTheme.SHOWA || hoppyTheme == HoppyTheme.MANGA

    override fun setPressed(pressed: Boolean) {
        val changed = pressed != isPressed
        super.setPressed(pressed)
        if (!changed) return
        if (pressesIntoShadow) {
            updateShineInsets()
            invalidate()
        } else if (hoppyTheme != HoppyTheme.PURIKURA && Motion.enabled()) {
            // (Purikura already squishes with the jelly loop.)
            animate().scaleX(if (pressed) 0.96f else 1f).scaleY(if (pressed) 0.96f else 1f).setDuration(90L).start()
        }
        if (pressed && hoppyTheme == HoppyTheme.MANGA && Motion.enabled()) {
            shakeTarget?.let { ButtonFx.shake(it, dp(5f)).start() }
        }
    }

    private fun pressOffset(): Float = if (pressesIntoShadow && isPressed) shadow else 0f

    // --- drawing -----------------------------------------------------------------------

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        pivotX = w / 2f
        pivotY = h / 2f
        faceShader = when (hoppyTheme) {
            HoppyTheme.PURIKURA -> LinearGradient(
                0f, faceStroke, 0f, h - shadow - faceStroke,
                color(R.color.purikura_btn_top), color(R.color.purikura_btn_bottom), Shader.TileMode.CLAMP,
            )
            HoppyTheme.YUME -> LinearGradient(
                margin, 0f, w - margin, 0f,
                intArrayOf(color(R.color.yume_btn_1), color(R.color.yume_btn_2), color(R.color.yume_btn_3)),
                null, Shader.TileMode.CLAMP,
            )
            else -> null
        }
        updateShineInsets()
    }

    /** The shine is clipped to the coloured face (inside border and ring). */
    private fun updateShineInsets() {
        val off = pressOffset()
        shine.insets.set(
            margin + faceStroke + off,
            margin + faceStroke + off,
            margin + faceStroke + shadowX() - off,
            margin + faceStroke + shadow - off,
        )
        shine.cornerRadius = cornerFor(height - 2 * (margin + faceStroke) - shadow)
        shine.invalidateGeometry()
    }

    private fun cornerFor(faceHeight: Float): Float = when (hoppyTheme) {
        HoppyTheme.PURIKURA, HoppyTheme.YUME -> faceHeight / 2f
        HoppyTheme.NIGHT -> dp(16f)
        else -> 0f
    }

    override fun onDraw(canvas: Canvas) {
        val w = width.toFloat()
        val h = height.toFloat()
        when (hoppyTheme) {
            HoppyTheme.PURIKURA -> drawPurikura(canvas, w, h)
            HoppyTheme.SHOWA -> drawSlab(canvas, w, h, color(R.color.showa_vermilion), color(R.color.showa_ink), color(R.color.showa_ink))
            HoppyTheme.MANGA -> drawSlab(canvas, w, h, color(R.color.manga_ink), color(R.color.manga_ink), color(R.color.manga_pink))
            HoppyTheme.YUME -> drawYume(canvas, w, h)
            HoppyTheme.NIGHT -> drawNight(canvas, w, h)
        }
        if (isFocused) drawFocusRing(canvas, w, h)
        val off = pressOffset()
        val save = canvas.save()
        canvas.translate(off, off)
        super.onDraw(canvas)
        canvas.restoreToCount(save)
    }

    private fun drawPurikura(canvas: Canvas, w: Float, h: Float) {
        val outerH = h - shadow
        val r = outerH / 2f
        facePaint.shader = null
        // Lavender drop below.
        facePaint.color = color(R.color.purikura_drop)
        canvas.drawRoundRect(0f, shadow, w, h, r, r, facePaint)
        // Outer ring.
        facePaint.color = color(R.color.purikura_primary_dark)
        face.set(0f, 0f, w, outerH)
        canvas.drawRoundRect(face, r, r, facePaint)
        // White border.
        facePaint.color = Color.WHITE
        tmp.set(face)
        tmp.inset(dp(2f), dp(2f))
        canvas.drawRoundRect(tmp, tmp.height() / 2f, tmp.height() / 2f, facePaint)
        // Gradient face.
        tmp.set(face)
        tmp.inset(faceStroke, faceStroke)
        facePaint.shader = faceShader
        canvas.drawRoundRect(tmp, tmp.height() / 2f, tmp.height() / 2f, facePaint)
        facePaint.shader = null
        // Small gloss on the upper half.
        facePaint.color = ColorUtils.setAlphaComponent(Color.WHITE, 70)
        gloss.set(tmp.left + tmp.height() * 0.4f, tmp.top + dp(3f), tmp.right - tmp.height() * 0.4f, tmp.centerY() - dp(2f))
        canvas.drawRoundRect(gloss, gloss.height() / 2f, gloss.height() / 2f, facePaint)
    }

    private fun drawSlab(canvas: Canvas, w: Float, h: Float, fill: Int, border: Int, shadowColor: Int) {
        val off = pressOffset()
        facePaint.shader = null
        if (off == 0f) {
            facePaint.color = shadowColor
            canvas.drawRect(shadow, shadow, w, h, facePaint)
        }
        face.set(off, off, w - shadow + off, h - shadow + off)
        facePaint.color = border
        canvas.drawRect(face, facePaint)
        facePaint.color = fill
        tmp.set(face)
        tmp.inset(faceStroke, faceStroke)
        canvas.drawRect(tmp, facePaint)
    }

    private fun drawYume(canvas: Canvas, w: Float, h: Float) {
        face.set(margin, margin, w - margin, h - margin)
        val r = face.height() / 2f
        facePaint.shader = null
        // Pulsing glow: a few soft rings outside the face.
        val lav = color(R.color.yume_mid)
        for (i in 1..3) {
            val spread = margin * i / 3f
            facePaint.color = ColorUtils.setAlphaComponent(lav, (glow * 40 / i).toInt().coerceIn(0, 255))
            tmp.set(face)
            tmp.inset(-spread, -spread)
            canvas.drawRoundRect(tmp, r + spread, r + spread, facePaint)
        }
        facePaint.shader = faceShader
        canvas.drawRoundRect(face, r, r, facePaint)
        facePaint.shader = null
        // Glossy top highlight.
        facePaint.color = ColorUtils.setAlphaComponent(Color.WHITE, 90)
        tmp.set(face.left + r * 0.6f, face.top + dp(3f), face.right - r * 0.6f, face.centerY() - dp(1f))
        canvas.drawRoundRect(tmp, tmp.height() / 2f, tmp.height() / 2f, facePaint)
        // White inner stroke (glass edge).
        facePaint.style = Paint.Style.STROKE
        facePaint.strokeWidth = dp(1.5f)
        facePaint.color = ColorUtils.setAlphaComponent(Color.WHITE, 170)
        tmp.set(face)
        tmp.inset(dp(1f), dp(1f))
        canvas.drawRoundRect(tmp, r, r, facePaint)
        facePaint.style = Paint.Style.FILL
    }

    private fun drawNight(canvas: Canvas, w: Float, h: Float) {
        face.set(0f, 0f, w, h)
        facePaint.shader = null
        facePaint.color = color(R.color.night_teal)
        val r = dp(16f)
        canvas.drawRoundRect(face, r, r, facePaint)
    }

    private fun drawFocusRing(canvas: Canvas, w: Float, h: Float) {
        facePaint.shader = null
        facePaint.style = Paint.Style.STROKE
        facePaint.strokeWidth = dp(2f)
        facePaint.color = ContextCompat.getColor(context, if (hoppyTheme.isDark) R.color.night_lavender else R.color.purikura_lavender)
        tmp.set(dp(1f), dp(1f), w - dp(1f), h - dp(1f))
        val r = cornerFor(h)
        canvas.drawRoundRect(tmp, r, r, facePaint)
        facePaint.style = Paint.Style.FILL
    }

    private companion object {
        val GLOW = object : android.util.FloatProperty<HoppyButton>("glow") {
            override fun setValue(obj: HoppyButton, value: Float) {
                obj.glow = value
                obj.invalidate()
            }

            override fun get(obj: HoppyButton): Float = obj.glow
        }
    }
}
