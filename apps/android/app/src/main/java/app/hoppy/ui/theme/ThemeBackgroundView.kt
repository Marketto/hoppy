package app.hoppy.ui.theme

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapShader
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.Shader
import android.util.AttributeSet
import android.view.View
import androidx.core.content.ContextCompat
import androidx.core.graphics.ColorUtils
import app.hoppy.R
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.sin

/**
 * Full-screen animated backdrop of the active theme (docs/design/themes.md).
 * Everything is drawn with Canvas; tiles and gradients are built once per size and
 * moved with shader matrices, so a frame allocates nothing.
 */
class ThemeBackgroundView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : View(context, attrs), HostAware {
    val theme: HoppyTheme = HoppyTheme.of(context)
    private val ticker = FrameTicker(this, fps = if (theme == HoppyTheme.YUME) 30 else 24)
    private var hostStarted = true

    private fun color(id: Int) = ContextCompat.getColor(context, id)

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val tilePaint = Paint(Paint.ANTI_ALIAS_FLAG or Paint.FILTER_BITMAP_FLAG)
    private val glowPaint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val shaderMatrix = Matrix()
    private var tile: Bitmap? = null
    private var tileSize = 0f
    private val starPath = Path()
    private val cloudPath = Path()
    private var gradientLength = 0f

    init {
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
        isClickable = false
        isFocusable = false
    }

    // --- lifecycle ---------------------------------------------------------------------

    override fun setHostStarted(started: Boolean) {
        hostStarted = started
        sync()
    }

    private fun sync() {
        ticker.update(animated && hostStarted && shouldAnimate())
    }

    private val animated: Boolean get() = theme != HoppyTheme.SHOWA

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        // Posted: isShown/windowVisibility are not yet updated during this callback.
        post { sync() }
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        sync()
    }

    override fun onDetachedFromWindow() {
        ticker.update(false)
        super.onDetachedFromWindow()
    }

    override fun onWindowVisibilityChanged(visibility: Int) {
        super.onWindowVisibilityChanged(visibility)
        sync()
    }

    // --- geometry ----------------------------------------------------------------------

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        if (w == 0 || h == 0) return
        tile?.recycle()
        tile = null
        when (theme) {
            HoppyTheme.PURIKURA -> buildCheckerTile()
            HoppyTheme.MANGA -> buildToneTile()
            HoppyTheme.YUME -> buildYume(w, h)
            HoppyTheme.NIGHT -> buildNight(w, h)
            HoppyTheme.SHOWA -> Unit
        }
    }

    /** Two 56dp squares per side (one period of the checkerboard) with white dots. */
    private fun buildCheckerTile() {
        val sq = dp(56f)
        tileSize = sq * 2
        val n = tileSize.toInt().coerceAtLeast(2)
        val bmp = Bitmap.createBitmap(n, n, Bitmap.Config.ARGB_8888)
        val c = Canvas(bmp)
        val s = n / 2f
        c.drawColor(color(R.color.purikura_check_b))
        fill.color = color(R.color.purikura_check_a)
        c.drawRect(0f, 0f, s, s, fill)
        c.drawRect(s, s, n.toFloat(), n.toFloat(), fill)
        fill.color = Color.WHITE
        val r = dp(3.5f)
        // Dots at the centre of every square.
        for (x in 0..1) for (y in 0..1) c.drawCircle(s * x + s / 2f, s * y + s / 2f, r, fill)
        tile = bmp
        tilePaint.shader = BitmapShader(bmp, Shader.TileMode.REPEAT, Shader.TileMode.REPEAT)
    }

    /** Screentone: grey dots on a 7dp grid. */
    private fun buildToneTile() {
        tileSize = dp(7f)
        val n = tileSize.toInt().coerceAtLeast(2)
        val bmp = Bitmap.createBitmap(n, n, Bitmap.Config.ARGB_8888)
        val c = Canvas(bmp)
        c.drawColor(color(R.color.manga_bg))
        fill.color = color(R.color.manga_tone)
        c.drawCircle(n / 2f, n / 2f, dp(1.1f), fill)
        tile = bmp
        tilePaint.shader = BitmapShader(bmp, Shader.TileMode.REPEAT, Shader.TileMode.REPEAT)
    }

    private fun buildYume(w: Int, h: Int) {
        gradientLength = hypot(w.toFloat(), h.toFloat())
        fill.shader = LinearGradient(
            0f, 0f, gradientLength, gradientLength * 0.35f,
            intArrayOf(
                color(R.color.yume_1), color(R.color.yume_2), color(R.color.yume_3),
                color(R.color.yume_4), color(R.color.yume_1),
            ),
            null, Shader.TileMode.MIRROR,
        )
        // Cloud: union of circles (non-zero winding), drawn with one alpha.
        cloudPath.reset()
        val u = dp(22f)
        cloudPath.addCircle(0f, 0f, u * 1.3f, Path.Direction.CW)
        cloudPath.addCircle(u * 1.4f, u * 0.3f, u, Path.Direction.CW)
        cloudPath.addCircle(-u * 1.4f, u * 0.4f, u * 0.9f, Path.Direction.CW)
        cloudPath.addRoundRect(-u * 2.2f, u * 0.2f, u * 2.3f, u * 1.2f, u * 0.5f, u * 0.5f, Path.Direction.CW)
        cloudPath.fillType = Path.FillType.WINDING
        buildStar()
    }

    private fun buildNight(w: Int, h: Int) {
        val teal = color(R.color.night_teal)
        glowPaint.shader = RadialGradient(
            w * 0.5f, h * 0.28f, w * 0.8f,
            intArrayOf(ColorUtils.setAlphaComponent(teal, 34), Color.TRANSPARENT),
            null, Shader.TileMode.CLAMP,
        )
        buildStar()
    }

    /** Unit 4-point star (radius 1), scaled when drawn. */
    private fun buildStar() {
        starPath.reset()
        starPath.moveTo(0f, -1f)
        starPath.quadTo(0.12f, -0.12f, 1f, 0f)
        starPath.quadTo(0.12f, 0.12f, 0f, 1f)
        starPath.quadTo(-0.12f, 0.12f, -1f, 0f)
        starPath.quadTo(-0.12f, -0.12f, 0f, -1f)
        starPath.close()
    }

    // --- drawing -----------------------------------------------------------------------

    override fun onDraw(canvas: Canvas) {
        val t = ticker.now()
        when (theme) {
            HoppyTheme.PURIKURA -> drawTiles(canvas, t, periodMs = 9000L, diagonal = true)
            HoppyTheme.MANGA -> drawTiles(canvas, t, periodMs = 6000L, diagonal = false)
            HoppyTheme.SHOWA -> drawShowa(canvas)
            HoppyTheme.YUME -> drawYume(canvas, t)
            HoppyTheme.NIGHT -> drawNight(canvas, t)
        }
    }

    private fun drawTiles(canvas: Canvas, t: Long, periodMs: Long, diagonal: Boolean) {
        if (tilePaint.shader == null) {
            canvas.drawColor(color(if (theme == HoppyTheme.MANGA) R.color.manga_bg else R.color.purikura_bg))
            return
        }
        val off = (t % periodMs).toFloat() / periodMs * tileSize
        shaderMatrix.reset()
        if (diagonal) shaderMatrix.setTranslate(off, off) else shaderMatrix.setTranslate(off, off * 0.5f)
        tilePaint.shader.setLocalMatrix(shaderMatrix)
        canvas.drawPaint(tilePaint)
    }

    private fun drawShowa(canvas: Canvas) {
        val w = width.toFloat()
        val h = height.toFloat()
        canvas.drawColor(color(R.color.showa_bg))
        // Top stripe band: vermilion / cream / teal, under the status bar.
        val band = dp(10f)
        fill.shader = null
        fill.color = color(R.color.showa_vermilion)
        canvas.drawRect(0f, 0f, w, band * 2.4f, fill)
        fill.color = color(R.color.showa_card)
        canvas.drawRect(0f, band * 2.4f, w, band * 2.9f, fill)
        fill.color = color(R.color.showa_teal)
        canvas.drawRect(0f, band * 2.9f, w, band * 3.6f, fill)
        // Halftone cluster, bottom-left: dots shrink away from the corner.
        fill.color = ColorUtils.setAlphaComponent(color(R.color.showa_vermilion), 70)
        val step = dp(12f)
        val maxR = step * 0.42f
        val reach = w * 0.55f
        var y = h
        while (y > h - reach) {
            var x = 0f
            while (x < reach) {
                val d = hypot(x, h - y) / reach
                val r = maxR * (1f - d)
                if (r > dp(0.6f)) canvas.drawCircle(x, y, r, fill)
                x += step
            }
            y -= step
        }
    }

    private fun drawYume(canvas: Canvas, t: Long) {
        val w = width.toFloat()
        val h = height.toFloat()
        // 1) Gradient sliding along its (mirrored) axis: ~16 s loop.
        val phase = (t % 16000L).toFloat() / 16000f
        shaderMatrix.reset()
        shaderMatrix.setTranslate(-gradientLength * 2f * phase, 0f)
        fill.shader?.setLocalMatrix(shaderMatrix)
        canvas.drawPaint(fill)

        // 2) Two cloud layers drifting horizontally (parallax).
        paint.shader = null
        paint.style = Paint.Style.FILL
        drawCloudLayer(canvas, t, w, h, periodMs = 70000L, alpha = 120, scale = 1.25f, rows = floatArrayOf(0.14f, 0.55f))
        drawCloudLayer(canvas, t, w, h, periodMs = 42000L, alpha = 170, scale = 0.9f, rows = floatArrayOf(0.32f, 0.82f))

        // 3) Twinkling 4-point stars.
        paint.color = Color.WHITE
        for (i in 0 until 7) {
            val x = w * rnd(i, 11)
            val y = h * (0.06f + 0.7f * rnd(i, 23))
            val tw = 0.5f + 0.5f * sin(2 * PI * ((t % 2600L) / 2600.0 + rnd(i, 5))).toFloat()
            drawStar(canvas, x, y, dp(4f + 5f * rnd(i, 7)) * (0.4f + 0.6f * tw), (90 + 165 * tw).toInt())
        }

        // 4) Bubbles rising and fading.
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(1.5f)
        for (i in 0 until 6) {
            val period = 9000L + (rnd(i, 3) * 6000).toLong()
            val p = ((t + (rnd(i, 17) * period).toLong()) % period).toFloat() / period
            val x = w * (0.08f + 0.84f * rnd(i, 29)) + dp(10f) * sin(p * 6f + i)
            val y = h * (1.05f - p * 0.9f)
            val r = dp(5f + 9f * rnd(i, 31))
            val a = (200 * (1f - p)).toInt().coerceIn(0, 255)
            paint.color = ColorUtils.setAlphaComponent(Color.WHITE, a)
            canvas.drawCircle(x, y, r, paint)
        }
        paint.style = Paint.Style.FILL
    }

    private fun drawCloudLayer(
        canvas: Canvas, t: Long, w: Float, h: Float, periodMs: Long, alpha: Int, scale: Float, rows: FloatArray,
    ) {
        paint.color = ColorUtils.setAlphaComponent(Color.WHITE, alpha)
        val span = w + dp(220f)
        val shift = (t % periodMs).toFloat() / periodMs * span
        rows.forEachIndexed { i, row ->
            val x = ((w * (0.2f + 0.5f * i) + shift) % span) - dp(110f)
            val save = canvas.save()
            canvas.translate(x, h * row)
            canvas.scale(scale, scale)
            canvas.drawPath(cloudPath, paint)
            canvas.restoreToCount(save)
        }
    }

    private fun drawStar(canvas: Canvas, x: Float, y: Float, r: Float, alpha: Int) {
        paint.alpha = alpha.coerceIn(0, 255)
        val save = canvas.save()
        canvas.translate(x, y)
        canvas.scale(r, r)
        canvas.drawPath(starPath, paint)
        canvas.restoreToCount(save)
    }

    private fun drawNight(canvas: Canvas, t: Long) {
        val w = width.toFloat()
        val h = height.toFloat()
        canvas.drawColor(color(R.color.night_bg))
        // Very faint teal glow, slowly breathing.
        val breath = 0.75f + 0.25f * sin(2 * PI * (t % 8000L) / 8000.0).toFloat()
        glowPaint.alpha = (255 * breath).toInt()
        canvas.drawRect(0f, 0f, w, h, glowPaint)
        // A few small twinkling stars.
        val lav = color(R.color.night_lavender)
        for (i in 0 until 9) {
            val x = w * rnd(i, 41)
            val y = h * 0.62f * rnd(i, 43)
            val tw = 0.5f + 0.5f * sin(2 * PI * ((t % 3400L) / 3400.0 + rnd(i, 47))).toFloat()
            paint.color = if (i % 3 == 0) lav else Color.WHITE
            if (i % 4 == 0) {
                drawStar(canvas, x, y, dp(3.5f), (40 + 150 * tw).toInt())
            } else {
                paint.alpha = (30 + 130 * tw).toInt()
                canvas.drawCircle(x, y, dp(1.2f), paint)
            }
        }
    }

    /** Deterministic pseudo-random 0..1 for decoration [i] / [salt]. */
    private fun rnd(i: Int, salt: Int): Float {
        val v = sin((i * 12.9898 + salt * 78.233)) * 43758.5453
        return abs(v - v.toLong()).toFloat()
    }
}
