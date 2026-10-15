package app.hoppy.ui

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.res.ColorStateList
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.util.Log
import android.view.Menu
import android.view.MenuItem
import android.view.animation.OvershootInterpolator
import android.widget.EditText
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import androidx.core.view.isVisible
import com.google.android.material.color.MaterialColors
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import app.hoppy.Node
import app.hoppy.Prefs
import app.hoppy.R
import app.hoppy.audio.Music
import app.hoppy.core.Peer
import app.hoppy.databinding.ActivityMainBinding
import app.hoppy.security.Security
import app.hoppy.service.ReceiveService
import app.hoppy.ui.theme.HomeDecor
import app.hoppy.ui.theme.Motion
import app.hoppy.update.Updater
import kotlin.concurrent.thread

/** Home: mascot, receive switch, nearby devices, "send files". Deliberately minimal. */
class MainActivity : ThemedActivity() {
    private lateinit var b: ActivityMainBinding
    private val adapter = PeerAdapter { peer -> pickFor(peer.device.id) }
    /** Device to send the picked files to (tapped in the list or a launcher shortcut). */
    private var targetPeerId: String? = null

    private val peersListener: (List<Peer>) -> Unit = { peers -> runOnUiThread { showPeers(peers) } }
    private val stateListener: () -> Unit = { runOnUiThread { syncSwitch() } }
    private val updateListener: () -> Unit = { runOnUiThread { syncUpdate() } }

    private val pickFiles = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        if (uris.isNotEmpty()) {
            startActivity(SendActivity.intent(this, uris, targetPeerId))
        }
        targetPeerId = null
    }

    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) {
        startReceiving()
    }

    /** Back from "Install unknown apps" for Hoppy: continue only if the user allowed it. */
    private val allowInstalls = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        if (packageManager.canRequestPackageInstalls()) Updater.download(this)
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
        b.updateButton.setOnClickListener { startUpdate() }
        decor = HomeDecor(b)
        styleSwitch()
        // Puts "Hoppy · nearby" in the share sheet's top row before the first send.
        val app = applicationContext
        thread(name = "hoppy-shortcuts") { ShareShortcuts.publishNearby(app) }
        if (savedInstanceState == null) {
            handleShortcut(intent)
            handleUpdate(intent)
        } else {
            targetPeerId = savedInstanceState.getString(EXTRA_PEER)
        }
    }

    override fun onSaveInstanceState(outState: Bundle) {
        super.onSaveInstanceState(outState)
        outState.putString(EXTRA_PEER, targetPeerId)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        if (::b.isInitialized) {
            handleShortcut(intent)
            handleUpdate(intent)
        }
    }

    /** Tap on our own "update available" notification (token-checked: this activity is exported). */
    private fun handleUpdate(intent: Intent) {
        if (Updater.isUpdateIntent(this, intent)) startUpdate()
    }

    /**
     * Download + install the newer release. Without "Install unknown apps" for Hoppy we
     * open that setting first and continue when the user comes back with it allowed.
     */
    private fun startUpdate() {
        if (Updater.availableVersion(this) == null) return
        if (!packageManager.canRequestPackageInstalls()) {
            val settings = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))
            if (runCatching { allowInstalls.launch(settings) }.isSuccess) return
        }
        Updater.download(this)
        syncUpdate()
    }

    private fun syncUpdate() {
        val version = Updater.availableVersion(this)
        b.updateBanner.isVisible = version != null
        if (version == null) return
        val busy = Updater.downloading
        b.updateText.text = if (busy) getString(R.string.update_downloading) else getString(R.string.update_banner, version)
        b.updateButton.isEnabled = !busy
    }

    /**
     * Launcher long-press shortcut ("Hoppy · nearby" or a device): open the picker. This
     * action is exported, so the device is only a preselection (SendActivity asks for a
     * tap) and only for a device we already sent to.
     */
    private fun handleShortcut(intent: Intent) {
        if (intent.action != ACTION_SEND_FILES) return
        pickFor(ShareTargets.recentPeer(intent.getStringExtra(EXTRA_PEER), Prefs.recentTargets(this)))
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
        Updater.addListener(updateListener)
        syncUpdate()
        Updater.maybeCheck(this)
        Node.acquire(this)
        Node.refresh()
        showPeers(Node.peers())
        syncSwitch()
        decor.start()
        showTrustWarning()
    }

    override fun onStop() {
        if (!::b.isInitialized) return super.onStop()
        decor.stop()
        Node.removeListener(peersListener)
        ReceiveService.removeStateListener(stateListener)
        Updater.removeListener(updateListener)
        Node.release()
        super.onStop()
    }

    private fun showPeers(peers: List<Peer>) {
        adapter.submitList(peers)
        b.empty.isVisible = peers.isEmpty()
        refreshTrustBadges(this, adapter)
    }

    /** A damaged list of known devices was set aside at startup: say so once. */
    private fun showTrustWarning() {
        val app = applicationContext
        Security.takeKeyProblem()?.let { Toast.makeText(app, it, Toast.LENGTH_LONG).show() }
        thread(name = "hoppy-trust-warning") {
            val warning = runCatching { Security.trust(app).takeWarning() }.getOrNull() ?: return@thread
            Log.w("HoppyMain", warning)
            runOnUiThread { Toast.makeText(app, R.string.trust_damaged, Toast.LENGTH_LONG).show() }
        }
    }

    private fun syncSwitch() {
        b.receiveSwitch.isChecked = ReceiveService.running
        b.receiveHint.setText(if (ReceiveService.running) R.string.receive_on_hint else R.string.receive_off_hint)
        decor.setReceiving(ReceiveService.running)
    }

    private fun pickFor(peerId: String?) {
        targetPeerId = peerId
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
        menu.findItem(R.id.music).isChecked = Music.enabled(this)
        return true
    }

    override fun onOptionsItemSelected(item: MenuItem): Boolean = when (item.itemId) {
        R.id.theme -> { chooseTheme(); true }
        R.id.rename -> { rename(); true }
        R.id.music -> {
            item.isChecked = !item.isChecked
            Music.setEnabled(this, item.isChecked)
            true
        }
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

    companion object {
        private const val ACTION_SEND_FILES = "app.hoppy.action.SEND_FILES"
        private const val EXTRA_PEER = "app.hoppy.PEER_ID"

        /** Intent of the share/launcher shortcuts: home + file picker (+ device). */
        fun sendFilesIntent(ctx: Context, peerId: String?): Intent =
            Intent(ctx, MainActivity::class.java)
                .setAction(ACTION_SEND_FILES)
                .apply { if (peerId != null) putExtra(EXTRA_PEER, peerId) }
    }
}
