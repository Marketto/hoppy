package app.hoppy.service

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import app.hoppy.core.FileSink
import app.hoppy.core.PendingFile
import app.hoppy.core.Protocol
import java.io.BufferedOutputStream
import java.io.IOException
import java.io.OutputStream

/**
 * Saves into Download/Hoppy through MediaStore (no storage permission needed).
 * Items stay IS_PENDING (invisible) until the hash is verified, like `.hoppy-part`.
 */
class MediaStoreSink(context: Context) : FileSink {
    private val resolver = context.applicationContext.contentResolver

    override fun create(name: String, size: Long): PendingFile {
        val ext = name.substringAfterLast('.', "").lowercase()
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, mime)
            put(MediaStore.Downloads.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/Hoppy")
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val uri: Uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: throw IOException("MediaStore refused $name")
        val raw = try {
            resolver.openOutputStream(uri) ?: throw IOException("cannot open $uri")
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw IOException("cannot write $name: ${e.message}")
        }
        return object : PendingFile {
            private var closed = false
            override val output: OutputStream = BufferedOutputStream(raw, Protocol.CHUNK)

            override fun commit(): String {
                output.close()
                closed = true
                resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null)
                return "${Environment.DIRECTORY_DOWNLOADS}/Hoppy/${displayName(uri) ?: name}"
            }

            override fun abort() {
                if (!closed) runCatching { output.close() }
                closed = true
                runCatching { resolver.delete(uri, null, null) }
            }
        }
    }

    /** MediaStore may rename on clashes ("foto (1).jpg"); report the real name. */
    private fun displayName(uri: Uri): String? =
        resolver.query(uri, arrayOf(MediaStore.Downloads.DISPLAY_NAME), null, null, null)?.use {
            if (it.moveToFirst()) it.getString(0) else null
        }
}
