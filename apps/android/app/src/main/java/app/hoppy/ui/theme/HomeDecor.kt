package app.hoppy.ui.theme

import android.animation.Animator
import android.animation.AnimatorSet
import android.animation.ObjectAnimator
import android.animation.PropertyValuesHolder
import android.animation.ValueAnimator
import android.content.Context
import android.graphics.drawable.GradientDrawable
import android.view.View
import android.view.animation.DecelerateInterpolator
import androidx.core.content.ContextCompat
import androidx.core.content.res.ResourcesCompat
import androidx.core.view.isVisible
import app.hoppy.R
import app.hoppy.databinding.ActivityMainBinding

/**
 * Per-theme decoration of the home screen (one layout, views toggled here):
 * Purikura "ぴょん！" sticker + washi-tape name; Showa logo ホッピー, header 近くの端末 and
 * the 受信中 stamp while receiving; Manga speech bubble, header 近くにいるよ and "ピョン!!";
 * Yume twinkling stars; Night こんばんは under the logo.
 * Call [start]/[stop] from onStart/onStop.
 */
class HomeDecor(private val b: ActivityMainBinding) {
    private val ctx = b.root.context
    val theme: HoppyTheme = HoppyTheme.of(ctx)
    private val loops = mutableListOf<Animator>()
    private var radar: Animator? = null
    private var started = false
    private var receiving = false

    private fun color(id: Int) = ContextCompat.getColor(ctx, id)

    init {
        ThemeFonts.applyDisplay(b.logo, b.headerDecor, b.nearbyHeader, b.sticker, b.pyonLabel, b.speechBubble, b.stampText)
        ThemeFonts.applyBodyBold(b.deviceName, b.receiveSwitch)
        b.logo.contentDescription = ctx.getString(R.string.app_name)

        when (theme) {
            HoppyTheme.PURIKURA -> {
                b.sticker.isVisible = true
                // Device name on a tilted yellow "washi tape" strip, in Yusei Magic.
                runCatching { ResourcesCompat.getFont(ctx, R.font.yusei_magic) }
                    .getOrNull()?.let { b.deviceName.typeface = it }
                b.deviceName.setBackgroundResource(R.drawable.decor_tape)
                b.deviceName.setPadding(ctx.px(14f), ctx.px(3f), ctx.px(14f), ctx.px(4f))
                b.deviceName.rotation = -3f
                b.logo.setTextColor(color(R.color.purikura_primary_dark))
            }
            HoppyTheme.SHOWA -> {
                b.logo.setText(R.string.decor_logo)
                b.logo.setTextColor(color(R.color.showa_teal))
                b.logo.setShadowLayer(1f, ctx.dp(3f), ctx.dp(3f), color(R.color.showa_mustard))
                header(R.string.decor_nearby_showa, fill = color(R.color.showa_vermilion), text = color(R.color.showa_card))
                b.radar1.alpha = 0f
                b.radar2.alpha = 0f
            }
            HoppyTheme.MANGA -> {
                b.speechBubble.isVisible = true
                b.pyonLabel.isVisible = true
                b.sendButton.shakeTarget = b.pyonLabel
                header(R.string.decor_nearby_manga, fill = color(R.color.manga_ink), text = color(R.color.manga_bg))
            }
            HoppyTheme.YUME -> {
                listOf(b.star1, b.star2, b.star3).forEach { it.isVisible = true }
                b.logo.setTextColor(color(R.color.yume_primary))
            }
            HoppyTheme.NIGHT -> {
                b.logoSub.isVisible = true
            }
        }
    }

    private fun header(textRes: Int, fill: Int, text: Int) {
        b.headerDecor.isVisible = true
        b.headerDecor.setText(textRes)
        b.headerDecor.setTextColor(text)
        b.headerDecor.background = GradientDrawable().apply { setColor(fill) }
    }

    fun start() {
        started = true
        if (!Motion.enabled()) return
        when (theme) {
            HoppyTheme.PURIKURA -> loops += ButtonFx.stickerPop(b.sticker, baseRotation = 8f)
            HoppyTheme.YUME -> {
                loops += ButtonFx.twinkle(b.star1, 1800L, 0L)
                loops += ButtonFx.twinkle(b.star2, 2200L, 500L)
                loops += ButtonFx.twinkle(b.star3, 2600L, 900L)
            }
            HoppyTheme.MANGA -> loops += ObjectAnimator.ofFloat(b.speechBubble, View.TRANSLATION_Y, 0f, -ctx.dp(4f)).apply {
                duration = 900L
                repeatCount = ValueAnimator.INFINITE
                repeatMode = ValueAnimator.REVERSE
            }
            else -> Unit
        }
        loops.forEach { it.start() }
        if (receiving) startRadar()
    }

    fun stop() {
        started = false
        loops.forEach { it.cancel() }
        loops.clear()
        stopRadar()
        listOf(b.sticker, b.star1, b.star2, b.star3).forEach {
            it.scaleX = 1f
            it.scaleY = 1f
            it.alpha = 1f
        }
        b.sticker.rotation = 8f
        b.speechBubble.translationY = 0f
    }

    /** Showa stamp "受信中" + radar rings, only while the receiver runs. */
    fun setReceiving(on: Boolean) {
        receiving = on
        if (theme != HoppyTheme.SHOWA) return
        b.stamp.isVisible = on
        if (on && started && Motion.enabled()) startRadar() else stopRadar()
    }

    private fun startRadar() {
        if (theme != HoppyTheme.SHOWA || radar != null) return
        fun ring(v: View, delay: Long) = ObjectAnimator.ofPropertyValuesHolder(
            v,
            PropertyValuesHolder.ofFloat(View.SCALE_X, 1f, 1.7f),
            PropertyValuesHolder.ofFloat(View.SCALE_Y, 1f, 1.7f),
            PropertyValuesHolder.ofFloat(View.ALPHA, 0.9f, 0f),
        ).apply {
            duration = 2000L
            startDelay = delay
            repeatCount = ValueAnimator.INFINITE
            interpolator = DecelerateInterpolator()
        }
        val set = AnimatorSet()
        set.playTogether(ring(b.radar1, 0L), ring(b.radar2, 1000L))
        set.start()
        radar = set
    }

    private fun stopRadar() {
        radar?.cancel()
        radar = null
        listOf(b.radar1, b.radar2).forEach {
            it.scaleX = 1f
            it.scaleY = 1f
            it.alpha = 0f
        }
    }

    private fun Context.px(v: Float): Int = dp(v).toInt()
}
