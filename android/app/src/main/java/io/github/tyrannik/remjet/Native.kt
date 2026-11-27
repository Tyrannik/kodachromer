package io.github.tyrannik.remjet

/**
 * The Rust side (crates/remjet-android). Every call is blocking and can take
 * seconds, so call it off the main thread. Failures throw RuntimeException
 * with a readable message.
 */
object Native {
    init {
        System.loadLibrary("remjet_android")
    }

    /** Films as "id\tname" lines, built-ins first. */
    @JvmStatic external fun filmList(): String

    /** A warning if the DNG isn't from a Leica M9 (colours may be off), else "". */
    @JvmStatic external fun cameraWarning(path: String): String

    /**
     * Preview JPEG of the DNG at [path] with [film] (null = neutral). [name] is
     * the photo's original file name, which seeds the grain.
     */
    @JvmStatic external fun renderPreview(path: String, name: String, film: String?): ByteArray

    /** Render at full or half size and write a JPEG to [out]. */
    @JvmStatic external fun export(path: String, name: String, film: String?, out: String, full: Boolean, quality: Int)
}
