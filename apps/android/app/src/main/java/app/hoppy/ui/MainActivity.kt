package app.hoppy.ui

import android.Manifest
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.os.Build
import android.os.Bundle
import android.view.Menu
import android.view.MenuItem
import android.view.animation.OvershootInterpolator
import android.widget.EditText
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.isVisible
import com.google.android.material.color.MaterialColors
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import app.hoppy.Node
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.core.Peer
import app.hoppy.databinding.ActivityMainBinding
import app.hoppy.service.ReceiveService
import app.hoppy.ui.theme.HomeDecor
import app.hoppy.ui.theme.Motion

/** Home: mascot, receive switch, nearby devices, "send files". Deliberately minimal. */
class MainActivity : ThemedActivity() {
    private lateinit var b: ActivityMainBinding
    private val adapter = PeerAdapter { peer -> pickFor(peer) }
    private var targetPeer: Peer? = null

    private val peersListener: (List<Peer>) -> Unit = { peers -> runOnUiThread { showPeers(peers) } }
    private val stateListener: () -> Unit = { runOnUiThread { syncSwitch() } }

    private val pickFiles = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) {
            startActivity(SendActivity.intent(this, uris, targetPeer?.device?.id))
        }
        targetPeer = null
    }

    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        startReceiving()
    }

    private lateinit var decor: HomeDecor

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (!Prefs.themeChosen(this)) {
            // First run: the theme choice comes before anything else (Android has no
            // install-time UI). The picker restarts MainActivity in the chosen theme.
            startActivity(ThemePickerActivity.intent(this, firstRun = true))
            finish()
            return
        }
        b = ActivityMainBinding.inflate(layoutInflater)
        setContentView(b.root)
        setSupportActionBar(b.toolbar)
        supportActionBar?.setDisplayShowTitleEnabled(false)

        b.peers.adapter = adapter
        b.sendButton.setOnClickListener { pickFor(null) }
        b.receiveSwitch.setOnCheckedChangeListener { _, checked ->
            if (checked == ReceiveService.running) return@setOnCheckedChangeListener
            springSwitch()
            if (checked) requestThenReceive() else ReceiveService.stop(this)
        }
        b.mascot.setOnClickListener { Node.refresh() }
        decor = HomeDecor(b)
        styleSwitch()
    }

    /** Thumb/track tinted per theme; the thumb springs (overshoot) when toggled. */
    private fun styleSwitch() {
        val sw = b.receiveSwitch
        val on = MaterialColors.getColor(sw, androidx.appcompat.R.attr.colorPrimary)
        val onThumb = MaterialColors.getColor(sw, com.google.android.material.R.attr.colorOnPrimary)
        val off = MaterialColors.getColor(sw, com.google.android.material.R.attr.colorOutline)
        val offTrack = MaterialColors.getColor(sw, com.google.android.material.R.attr.colorSurfaceContainerHighest)
        val checked = intArrayOf(android.R.attr.state_checked)
        val unchecked = intArrayOf()
        sw.thumbTintList = ColorStateList(arrayOf(checked, unchecked), intArrayOf(onThumb, off))
        sw.trackTintList = ColorStateList(arrayOf(checked, unchecked), intArrayOf(on, offTrack))
        sw.trackDecorationTintList = ColorStateList(arrayOf(checked, unchecked), intArrayOf(on, off))
    }

    /** Springy feedback on the whole switch (the Material thumb itself is not animatable from outside). */
    private fun springSwitch() {
        if (!Motion.enabled()) return
        val sw = b.receiveSwitch
        sw.animate().cancel()
        sw.scaleX = 0.96f
        sw.scaleY = 0.96f
        sw.animate().scaleX(1f).scaleY(1f).setDuration(380L).setInterpolator(OvershootInterpolator(3f)).start()
    }

    override fun onStart() {
        super.onStart()
        if (!::b.isInitialized) return // first run: finishing towards the theme picker
        b.deviceName.text = Prefs.deviceName(this)
        Node.addListener(peersListener)
        ReceiveService.addStateListener(stateListener)
        Node.acquire(this)
        Node.refresh()
        showPeers(Node.peers())
        syncSwitch()
        decor.start()
    }

    override fun onStop() {
        if (!::b.isInitialized) return super.onStop()
        decor.stop()
        Node.removeListener(peersListener)
        ReceiveService.removeStateListener(stateListener)
        Node.release()
        super.onStop()
    }

    private fun showPeers(peers: List<Peer>) {
        adapter.submitList(peers)
        b.empty.isVisible = peers.isEmpty()
    }

    private fun syncSwitch() {
        b.receiveSwitch.isChecked = ReceiveService.running
        b.receiveHint.setText(if (ReceiveService.running) R.string.receive_on_hint else R.string.receive_off_hint)
        decor.setReceiving(ReceiveService.running)
    }

    private fun pickFor(peer: Peer?) {
        targetPeer = peer
        pickFiles.launch(arrayOf("*/*"))
    }

    private fun requestThenReceive() {
        val needs = Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        if (needs) askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS) else startReceiving()
    }

    private fun startReceiving() {
        ReceiveService.start(this)
    }

    override fun onCreateOptionsMenu(menu: Menu): Boolean {
        menuInflater.inflate(R.menu.main, menu)
        menu.findItem(R.id.auto_accept).isChecked = Prefs.autoAccept(this)
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean = when (item.itemId) {
        R.id.theme -> { chooseTheme(); true }
        R.id.rename -> { rename(); true }
        R.id.auto_accept -> {
            val value = !item.isChecked
            if (value) {
                MaterialAlertDialogBuilder(this)
                    .setTitle(R.string.auto_accept)
                    .setMessage(R.string.auto_accept_warning)
                    .setNegativeButton(android.R.string.cancel, null)
                    .setPositiveButton(android.R.string.ok) { _, _ ->
                        Prefs.setAutoAccept(this, true)
                        item.isChecked = true
                    }
                    .show()
            } else {
                Prefs.setAutoAccept(this, false)
                item.isChecked = false
            }
            true
        }
        else -> super.onOptionsItemSelected(item)
    }

    private fun chooseTheme() {
        startActivity(ThemePickerActivity.intent(this, firstRun = false))
    }

    private fun rename() {
        val input = EditText(this).apply {
            setText(Prefs.deviceName(this@MainActivity))
            setSelection(text.length)
            isSingleLine = true
        }
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.rename)
            .setView(input)
            .setNegativeButton(android.R.string.cancel, null)
            .setPositiveButton(android.R.string.ok) { _, _ ->
                val name = input.text.toString().trim()
                if (name.isNotEmpty()) {
                    Prefs.setDeviceName(this, name)
                    b.deviceName.text = Prefs.deviceName(this)
                    Node.restart(this)
                }
            }
            .show()
    }
}
