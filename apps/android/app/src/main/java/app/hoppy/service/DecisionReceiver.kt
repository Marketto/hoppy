package app.hoppy.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Accept/Decline buttons of the incoming-transfer notification. */
class DecisionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getIntExtra(EXTRA_ID, -1)
        val accept = intent.getBooleanExtra(EXTRA_ACCEPT, false)
        ReceiveService.pending[id]?.complete(accept)
    }

    companion object {
        const val EXTRA_ID = "id"
        const val EXTRA_ACCEPT = "accept"
    }
}
