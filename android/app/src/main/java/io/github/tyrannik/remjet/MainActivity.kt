package io.github.tyrannik.remjet

import android.app.Activity
import android.app.ActivityManager
import android.app.AlertDialog
import android.content.ContentValues
import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.view.View
import android.widget.AdapterView
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.ImageView
import android.widget.ProgressBar
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast
import java.io.File
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var preview: ImageView
    private lateinit var empty: TextView
    private lateinit var busy: ProgressBar
    private lateinit var status: TextView
    private lateinit var filmSpinner: Spinner
    private lateinit var sizeSpinner: Spinner
    private lateinit var openButton: Button
    private lateinit var samplesButton: Button
    private lateinit var exportButton: Button

    /** One worker thread: renders run in order and never overlap. */
    private val worker = Executors.newSingleThreadExecutor()

    /** (id, name); id "" is the neutral look. */
    private var films: List<Pair<String, String>> = emptyList()

    /** The open photo, copied into the cache folder. */
    private var dng: File? = null
    private var dngName: String = ""

    /** Bumped on every preview request so stale results are dropped. */
    private var previewGeneration = 0

    /** A finished export waiting for the "save as" picker (Android 9 and older). */
    private var pendingExport: File? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        preview = findViewById(R.id.preview)
        empty = findViewById(R.id.empty)
        busy = findViewById(R.id.busy)
        status = findViewById(R.id.status)
        filmSpinner = findViewById(R.id.film)
        sizeSpinner = findViewById(R.id.size)
        openButton = findViewById(R.id.open)
        samplesButton = findViewById(R.id.samples)
        exportButton = findViewById(R.id.export)

        films = listOf("" to getString(R.string.neutral)) +
            Native.filmList().lines().filter { it.contains('\t') }.map { line ->
                val (id, name) = line.split('\t', limit = 2)
                id to name
            }
        filmSpinner.adapter = spinnerAdapter(films.map { it.second })
        // Start on Kodachrome 64, like the desktop app.
        filmSpinner.setSelection(films.indexOfFirst { it.first == "kodachrome64" }.coerceAtLeast(0))
        filmSpinner.onItemSelectedListener = object : AdapterView.OnItemSelectedListener {
            override fun onItemSelected(parent: AdapterView<*>?, view: View?, position: Int, id: Long) = refreshPreview()
            override fun onNothingSelected(parent: AdapterView<*>?) {}
        }

        sizeSpinner.adapter = spinnerAdapter(listOf(getString(R.string.size_full), getString(R.string.size_half)))
        // Half size by default on phones with little memory.
        sizeSpinner.setSelection(if (totalMemoryMb() < 3000) 1 else 0)

        openButton.setOnClickListener { pickDng() }
        // Builds from CI bundle a few public-domain M9 DNGs for testing.
        val samples = bundledSamples()
        samplesButton.visibility = if (samples.isEmpty()) View.GONE else View.VISIBLE
        samplesButton.setOnClickListener { pickSample(samples) }
        exportButton.setOnClickListener { confirmExport() }

        intent?.let { handleIncoming(it) }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIncoming(intent)
    }

    override fun onDestroy() {
        worker.shutdownNow()
        super.onDestroy()
    }

    private fun spinnerAdapter(items: List<String>) =
        ArrayAdapter(this, android.R.layout.simple_spinner_item, items).apply {
            setDropDownViewResource(android.R.layout.simple_spinner_dropdown_item)
        }

    private fun selectedFilm(): String? = films.getOrNull(filmSpinner.selectedItemPosition)?.first?.ifEmpty { null }

    private fun fullSize() = sizeSpinner.selectedItemPosition == 0

    // ------------------------------------------------------------ opening

    private fun handleIncoming(intent: Intent) {
        val uri: Uri? = when (intent.action) {
            Intent.ACTION_VIEW -> intent.data
            Intent.ACTION_SEND -> if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION") intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }
            else -> null
        }
        uri?.let { openUri(it) }
    }

    private fun pickDng() {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            // DNGs are labelled inconsistently (image/x-adobe-dng, image/dng,
            // application/octet-stream...), so allow anything and check the
            // file when it is opened.
            type = "*/*"
        }
        startActivityForResult(intent, REQUEST_OPEN)
    }

    private fun openUri(uri: Uri) {
        val name = displayName(uri) ?: "photo.dng"
        openFrom(name) { contentResolver.openInputStream(uri) }
    }

    private fun bundledSamples(): List<String> =
        (assets.list(SAMPLES_DIR) ?: emptyArray()).filter { it.endsWith(".dng", ignoreCase = true) }.sorted()

    private fun pickSample(samples: List<String>) {
        AlertDialog.Builder(this)
            .setTitle("Sample DNGs (public domain, raw.pixls.us)")
            .setItems(samples.toTypedArray()) { _, which ->
                val name = samples[which]
                openFrom(name) { assets.open("$SAMPLES_DIR/$name") }
            }
            .show()
    }

    /** Copy a DNG from [open] into the cache folder and show it. */
    private fun openFrom(name: String, open: () -> java.io.InputStream?) {
        setBusy(true, "Opening $name…")
        worker.execute {
            try {
                // The core decodes from a file path, so copy the DNG into the
                // cache folder (an M9 DNG is 18–36 MB).
                val file = File(cacheDir, "open.dng")
                open().use { input ->
                    requireNotNull(input) { "Couldn't read $name" }
                    file.outputStream().use { input.copyTo(it) }
                }
                val warning = Native.cameraWarning(file.path)
                runOnUiThread {
                    dng = file
                    dngName = name
                    exportButton.isEnabled = true
                    empty.visibility = View.GONE
                    if (warning.isNotEmpty()) toast(warning)
                    refreshPreview()
                }
            } catch (e: Exception) {
                runOnUiThread { fail("Couldn't open $name", e) }
            }
        }
    }

    private fun displayName(uri: Uri): String? = try {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    } catch (e: Exception) {
        null
    } ?: uri.lastPathSegment?.substringAfterLast('/')

    // ------------------------------------------------------------ preview

    private fun refreshPreview() {
        val file = dng ?: return
        val name = dngName
        val film = selectedFilm()
        val generation = ++previewGeneration
        val filmName = films.getOrNull(filmSpinner.selectedItemPosition)?.second ?: ""
        setBusy(true, "Rendering $filmName…")
        worker.execute {
            if (generation != previewGeneration) return@execute // superseded
            try {
                val start = System.currentTimeMillis()
                val jpeg = Native.renderPreview(file.path, name, film)
                val bitmap = BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size)
                val ms = System.currentTimeMillis() - start
                runOnUiThread {
                    if (generation == previewGeneration) {
                        preview.setImageBitmap(bitmap)
                        setBusy(false, "$name · $filmName · preview in ${ms} ms")
                    }
                }
            } catch (e: Exception) {
                runOnUiThread { fail("Preview failed", e) }
            }
        }
    }

    // ------------------------------------------------------------ export

    private fun confirmExport() {
        // A full-size render holds several 18 MP float images at once. On a
        // phone that is short of memory Android would just kill the app, so
        // warn first.
        val freeMb = availableMemoryMb()
        if (fullSize() && freeMb < FULL_EXPORT_MB) {
            AlertDialog.Builder(this)
                .setTitle("Low memory")
                .setMessage("Full size needs about $FULL_EXPORT_MB MB and this phone has $freeMb MB free. It may close the app. Export at half size instead?")
                .setPositiveButton("Half size") { _, _ ->
                    sizeSpinner.setSelection(1)
                    export(full = false)
                }
                .setNegativeButton("Try full size") { _, _ -> export(full = true) }
                .show()
        } else {
            export(fullSize())
        }
    }

    private fun exportName(): String {
        val stem = dngName.substringBeforeLast('.')
        return "${stem}_${selectedFilm() ?: "neutral"}.jpg"
    }

    private fun export(full: Boolean) {
        val file = dng ?: return
        val name = dngName
        val film = selectedFilm()
        val outName = exportName()
        setBusy(true, "Exporting $outName at ${if (full) "full" else "half"} size…")
        exportButton.isEnabled = false
        worker.execute {
            try {
                val start = System.currentTimeMillis()
                val out = File(cacheDir, "export.jpg")
                Native.export(file.path, name, film, out.path, full, JPEG_QUALITY)
                val seconds = (System.currentTimeMillis() - start) / 1000.0
                if (Build.VERSION.SDK_INT >= 29) {
                    saveToPictures(out, outName)
                    runOnUiThread {
                        exportButton.isEnabled = true
                        setBusy(false, "Saved Pictures/Remjet/$outName (%.1f s)".format(seconds))
                        toast("Saved to Pictures/Remjet")
                    }
                } else {
                    runOnUiThread {
                        exportButton.isEnabled = true
                        setBusy(false, "Rendered in %.1f s — choose where to save".format(seconds))
                        pendingExport = out
                        saveAs(outName)
                    }
                }
            } catch (e: Exception) {
                runOnUiThread {
                    exportButton.isEnabled = true
                    fail("Export failed", e)
                }
            }
        }
    }

    /** Android 10+: add the JPEG to Pictures/Remjet, no permission needed. */
    private fun saveToPictures(jpeg: File, name: String) {
        val values = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, name)
            put(MediaStore.Images.Media.MIME_TYPE, "image/jpeg")
            put(MediaStore.Images.Media.RELATIVE_PATH, Environment.DIRECTORY_PICTURES + "/Remjet")
            put(MediaStore.Images.Media.IS_PENDING, 1)
        }
        val resolver = contentResolver
        val uri = requireNotNull(resolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)) {
            "Couldn't create the file in Pictures"
        }
        try {
            resolver.openOutputStream(uri).use { out ->
                requireNotNull(out) { "Couldn't write to Pictures" }
                jpeg.inputStream().use { it.copyTo(out) }
            }
            values.clear()
            values.put(MediaStore.Images.Media.IS_PENDING, 0)
            resolver.update(uri, values, null, null)
        } catch (e: Exception) {
            resolver.delete(uri, null, null)
            throw e
        } finally {
            jpeg.delete()
        }
    }

    /** Android 9 and older: let the user pick where to save (no permission needed). */
    private fun saveAs(name: String) {
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = "image/jpeg"
            putExtra(Intent.EXTRA_TITLE, name)
        }
        startActivityForResult(intent, REQUEST_SAVE)
    }

    @Deprecated("Framework Activity API; fine without AndroidX")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        val uri = data?.data
        when (requestCode) {
            REQUEST_OPEN -> if (resultCode == RESULT_OK && uri != null) openUri(uri)
            REQUEST_SAVE -> {
                val jpeg = pendingExport ?: return
                pendingExport = null
                if (resultCode != RESULT_OK || uri == null) {
                    jpeg.delete()
                    return
                }
                worker.execute {
                    try {
                        contentResolver.openOutputStream(uri).use { out ->
                            requireNotNull(out) { "Couldn't write the file" }
                            jpeg.inputStream().use { it.copyTo(out) }
                        }
                        runOnUiThread { toast("Saved") }
                    } catch (e: Exception) {
                        runOnUiThread { fail("Save failed", e) }
                    } finally {
                        jpeg.delete()
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------ helpers

    private fun memoryInfo() = ActivityManager.MemoryInfo().also {
        (getSystemService(ACTIVITY_SERVICE) as ActivityManager).getMemoryInfo(it)
    }

    private fun availableMemoryMb() = (memoryInfo().availMem / (1024 * 1024)).toInt()

    private fun totalMemoryMb() = (memoryInfo().totalMem / (1024 * 1024)).toInt()

    private fun setBusy(on: Boolean, message: String) {
        busy.visibility = if (on) View.VISIBLE else View.GONE
        openButton.isEnabled = !on
        samplesButton.isEnabled = !on
        status.setTextColor(getColor(R.color.muted))
        status.text = message
    }

    private fun fail(what: String, e: Exception) {
        setBusy(false, "")
        status.setTextColor(getColor(R.color.warning))
        status.text = "$what: ${e.message ?: e.javaClass.simpleName}"
    }

    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_LONG).show()

    companion object {
        private const val REQUEST_OPEN = 1
        private const val REQUEST_SAVE = 2
        private const val JPEG_QUALITY = 92
        private const val SAMPLES_DIR = "samples"

        /** Peak memory of a full-size (18 MP) export, measured at ~960 MB. */
        private const val FULL_EXPORT_MB = 1100
    }
}
