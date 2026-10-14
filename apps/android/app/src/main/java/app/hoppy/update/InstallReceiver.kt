package app.hoppy.update

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import app.hoppy.R

/**
 * Result of the PackageInstaller session committed by [Updater]. Not exported: only our
 * own PendingIntent (filled in by the system installer) reaches it.
 */
class InstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION_STATUS) return
        when (val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                val confirm = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                }
                if (confirm == null) {
                    Updater.fail(context, context.getString(R.string.update_install_failed))
                } else {
                    Updater.confirm(context, confirm)
                }
            }
            // Success: the process is about to be replaced. Aborted: the user said no.
            PackageInstaller.STATUS_SUCCESS, PackageInstaller.STATUS_FAILURE_ABORTED ->
                Updater.cancel(context, Updater.NOTIF_PROGRESS)
            else -> {
                val msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                Updater.fail(context, msg ?: "${context.getString(R.string.update_install_failed)} ($status)")
            }
        }
    }

    companion object {
        const val ACTION_STATUS = "app.hoppy.action.INSTALL_STATUS"
    }
}
