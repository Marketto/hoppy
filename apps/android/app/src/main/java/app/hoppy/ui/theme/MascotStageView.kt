package app.hoppy.ui.theme

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RadialGradient
import android.graphics.RectF
import android.graphics.Shader
import android.util.AttributeSet
import android.widget.FrameLayout
import androidx.core.content.ContextCompat
import androidx.core.graphics.ColorUtils
import app.hoppy.R
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin

/**
 * FrameLayout around [MascotView] that paints the theme's stage behind it:
 * Purikura sticker + pulse rings, Showa rotating sunburst, Manga speed lines in an
 * ink panel, Yume floating glossy bubble, Night breathing teal halo.
 */
class MascotStageView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : FrameLayout(context, attrs), HostAware {
    val hoppyTheme: HoppyTheme = HoppyTheme.of(context)
    private val ticker = FrameTicker(this, fps = 30)
    private var hostStarted = true

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val shaded = Paint(Paint.ANTI_ALIAS_FLAG)
    private val path = Path()
    private val rect = RectF()
    private var cx = 0f
    private var cy = 0f
    private var radius = 0f

    init {
        setWillNotDraw(false)
        clipChildren = false
        clipToPadding = false
    }

    private fun color(id: Int) = ContextCompat.getColor(context, id)

    override fun setHostStarted(started: Boolean) {
        hostStarted = started
        sync()
    }

    private fun sync() = ticker.update(hostStarted && shouldAnimate())

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

    override fun onVisibilityAggregated(isVisible: Boolean) {
        super.onVisibilityAggregated(isVisible)
        post { sync() }
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        cx = w / 2f
        cy = h / 2f
        radius = min(w, h) / 2f - dp(6f)
        when (hoppyTheme) {
            HoppyTheme.MANGA -> shaded.shader = RadialGradient(
                cx, cy, radius * 0.75f,
                intArrayOf(Color.WHITE, Color.WHITE, Color.TRANSPARENT), floatArrayOf(0f, 0.55f, 1f),
                Shader.TileMode.CLAMP,
            )
            HoppyTheme.NIGHT -> shaded.shader = RadialGradient(
                cx, cy, radius,
                intArrayOf(ColorUtils.setAlphaComponent(color(R.color.night_teal), 90), Color.TRANSPARENT),
                null, Shader.TileMode.CLAMP,
            )
            HoppyTheme.YUME -> shaded.shader = RadialGradient(
                cx - radius * 0.3f, cy - radius * 0.35f, radius * 1.3f,
                intArrayOf(
                    ColorUtils.setAlphaComponent(Color.WHITE, 200),
                    ColorUtils.setAlphaComponent(color(R.color.yume_1), 110),
                    ColorUtils.setAlphaComponent(color(R.color.yume_pink), 80),
                ),
                floatArrayOf(0f, 0.6f, 1f), Shader.TileMode.CLAMP,
            )
            HoppyTheme.SHOWA -> shaded.shader = null
            HoppyTheme.PURIKURA -> shaded.shader = null
        }
    }

    override fun onDraw(canvas: Canvas) {
        if (radius <= 0f) return
        val t = ticker.now()
        when (hoppyTheme) {
            HoppyTheme.PURIKURA -> drawPurikura(canvas, t)
            HoppyTheme.SHOWA -> drawShowa(canvas, t)
            HoppyTheme.MANGA -> drawManga(canvas, t)
            HoppyTheme.YUME -> drawYume(canvas, t)
            HoppyTheme.NIGHT -> drawNight(canvas, t)
        }
    }

    private fun drawPurikura(canvas: Canvas, t: Long) {
        val pink = color(R.color.purikura_primary)
        val r = radius * 0.86f
        // Two pulse rings expanding and fading, half a period apart.
        stroke.strokeWidth = dp(3f)
        for (k in 0..1) {
            val p = ((t + k * 1300L) % 2600L).toFloat() / 2600f
            stroke.color = ColorUtils.setAlphaComponent(pink, (150 * (1f - p)).toInt())
            canvas.drawCircle(cx, cy, r + (radius - r + dp(4f)) * p * 1.6f, stroke)
        }
        fill.shader = null
        fill.color = Color.WHITE
        canvas.drawCircle(cx, cy, r, fill)
        stroke.color = color(R.color.purikura_ring)
        stroke.strokeWidth = dp(4f)
        canvas.drawCircle(cx, cy, r - dp(2f), stroke)
    }

    private fun drawShowa(canvas: Canvas, t: Long) {
        val r = radius * 0.94f
        fill.shader = null
        fill.color = color(R.color.showa_card)
        canvas.drawCircle(cx, cy, r, fill)
        // Sunburst: 16 mustard wedges, one turn per ~26 s.
        val angle = (t % 26000L).toFloat() / 26000f * 360f
        val save = canvas.save()
        path.reset()
        path.addCircle(cx, cy, r, Path.Direction.CW)
        canvas.clipPath(path)
        canvas.rotate(angle, cx, cy)
        fill.color = color(R.color.showa_mustard)
        val rays = 16
        val half = (PI / rays / 2).toFloat()
        path.reset()
        for (i in 0 until rays) {
            val a = (2 * PI * i / rays).toFloat()
            path.moveTo(cx, cy)
            path.lineTo(cx + cos(a - half) * r * 1.2f, cy + sin(a - half) * r * 1.2f)
            path.lineTo(cx + cos(a + half) * r * 1.2f, cy + sin(a + half) * r * 1.2f)
            path.close()
        }
        canvas.drawPath(path, fill)
        canvas.restoreToCount(save)
        stroke.color = color(R.color.showa_ink)
        stroke.strokeWidth = dp(4f)
        canvas.drawCircle(cx, cy, r - dp(2f), stroke)
    }

    private fun drawManga(canvas: Canvas, t: Long) {
        val ink = color(R.color.manga_ink)
        val inset = dp(4f)
        rect.set(inset, inset, width - inset, height - inset)
        // Hard offset shadow, then the white panel.
        fill.shader = null
        fill.color = ink
        canvas.drawRect(rect.left + dp(4f), rect.top + dp(4f), rect.right + dp(4f), rect.bottom + dp(4f), fill)
        fill.color = Color.WHITE
        canvas.drawRect(rect, fill)
        // Speed lines: thin rays from the centre, ~20 s per turn, slight flicker.
        val save = canvas.save()
        canvas.clipRect(rect)
        val angle = (t % 20000L).toFloat() / 20000f * 360f
        canvas.rotate(angle, cx, cy)
        val flicker = ((t / 90L) % 3L).toInt()
        val reach = radius * 2.2f
        val rays = 48
        stroke.color = ink
        for (i in 0 until rays) {
            if ((i + flicker) % 7 == 0) continue
            val a = (2 * PI * i / rays).toFloat()
            stroke.strokeWidth = dp(if (i % 3 == 0) 1.6f else 0.9f)
            val start = radius * (0.42f + 0.08f * ((i * 5) % 3))
            canvas.drawLine(cx + cos(a) * start, cy + sin(a) * start, cx + cos(a) * reach, cy + sin(a) * reach, stroke)
        }
        canvas.restoreToCount(save)
        // White radial clearing behind the bunny.
        canvas.drawCircle(cx, cy, radius * 0.75f, shaded)
        stroke.strokeWidth = dp(4f)
        canvas.drawRect(rect, stroke)
    }

    private fun drawYume(canvas: Canvas, t: Long) {
        val floatY = dp(6f) * sin(2 * PI * (t % 4000L) / 4000.0).toFloat()
        val r = radius * 0.9f
        val save = canvas.save()
        canvas.translate(0f, floatY)
        canvas.drawCircle(cx, cy, r, shaded)
        stroke.color = ColorUtils.setAlphaComponent(Color.WHITE, 220)
        stroke.strokeWidth = dp(2f)
        canvas.drawCircle(cx, cy, r, stroke)
        // Rotating highlight arc.
        val angle = (t % 9000L).toFloat() / 9000f * 360f
        stroke.strokeWidth = dp(5f)
        stroke.strokeCap = Paint.Cap.ROUND
        stroke.color = ColorUtils.setAlphaComponent(Color.WHITE, 200)
        rect.set(cx - r + dp(9f), cy - r + dp(9f), cx + r - dp(9f), cy + r - dp(9f))
        canvas.drawArc(rect, 200f + angle, 50f, false, stroke)
        stroke.strokeCap = Paint.Cap.BUTT
        canvas.restoreToCount(save)
    }

    private fun drawNight(canvas: Canvas, t: Long) {
        val breath = 0.7f + 0.3f * sin(2 * PI * (t % 5000L) / 5000.0).toFloat()
        val save = canvas.save()
        canvas.scale(0.9f + 0.1f * breath, 0.9f + 0.1f * breath, cx, cy)
        shaded.alpha = (255 * breath).toInt()
        canvas.drawCircle(cx, cy, radius, shaded)
        canvas.restoreToCount(save)
        stroke.color = ColorUtils.setAlphaComponent(color(R.color.night_teal), 60)
        stroke.strokeWidth = dp(1.5f)
        canvas.drawCircle(cx, cy, radius * 0.82f, stroke)
    }
}
