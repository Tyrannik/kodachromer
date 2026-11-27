# Remjet for Android (experimental)

A small companion app for the phone: open an M9 DNG, pick a film, export a JPEG. It runs the
same `remjet-core` as the desktop app, so a film looks the same on both. There are no
adjustments yet (exposure, warmth and so on use the desktop defaults).

## What your phone needs

- **Android 7.0 or newer** (API 24), with an ARM processor (32- or 64-bit) or a 64-bit x86 one.
  That covers almost every Android phone from 2016 onwards, plus emulators such as BlueStacks and
  the Android Studio emulator.
- **Memory:** the preview needs about 330 MB. A **half-size** export (4.5 MP, 2606 × 1736) needs
  about the same. A **full-size** export (18 MP) peaks at about 1 GB, so on a phone with 2–3 GB of
  RAM half size is the safer choice; the app picks it by default on phones under 3 GB and warns
  before a full-size export when memory is short.
- **Speed:** on a desktop CPU a preview takes 0.3 s, a half-size export 0.7 s and a full-size
  export 2.7 s. Expect an old phone to be several times slower: a few seconds per film change and
  up to half a minute for a full-size export.

## Installing it

The app isn't on the Play Store, so you install the APK file yourself ("sideloading"). It's free.

1. On the phone, open the [Releases page](https://github.com/Tyrannik/remjet/releases) and download
   `Remjet-<version>-android.apk`. (Builds between releases are on the
   [Android workflow](https://github.com/Tyrannik/remjet/actions/workflows/android.yml) runs as a
   zip: open the latest green run, download the artifact under *Artifacts*, and unzip it.
   Artifacts need a GitHub login.)
2. Open the downloaded APK, from the browser's download notification or the Files app.
3. Android asks to allow installing from this source: tap **Settings**, turn on **Allow from this
   source** for your browser or Files app, then go back and tap **Install**. (On Android 7, the
   switch is *Settings → Security → Unknown sources*.)
4. Play Protect may say the app is from an unknown developer: tap **More details → Install
   anyway**. It warns because the app isn't from the Play Store.

Newer builds install over the old one (they share a signing key), so you don't need to
uninstall first.

**BlueStacks:** drag the APK onto the BlueStacks window (or use *Install APK* in its side bar).
Use a 64-bit instance (Android 9 or 11 "64-bit"); give it 4 GB of memory for full-size exports.
Exports land in BlueStacks' own Pictures folder; its *Media Manager* exports them to Windows.

**From a Windows PC instead:** copy the APK to the phone over USB and open it there, or with
USB debugging on, run `adb install -r Remjet-<version>-android.apk`.

## Using it

1. **Open DNG** opens the system file picker. DNGs from a card reader, a USB-C stick, Google
   Drive or the phone's storage all work. You can also share a DNG to Remjet, or choose
   *Open with → Remjet* in a file manager.
   **Samples** opens one of the public-domain DNGs from [raw.pixls.us](https://raw.pixls.us) that
   come with the app (an M9 plus other Leica bodies, as raw.pixls.us has only one M9 file), handy
   for trying it on a device without copying photos across.
2. Choose a **Film** (or *Neutral*). The preview updates.
3. Choose a **Size** and tap **Export JPEG**. On Android 10 and newer the JPEG goes to
   *Pictures/Remjet* and shows up in the gallery; on older versions you choose where to save it.
   The file is named `<photo>_<film>.jpg`, keeps the camera's EXIF data and records the film look,
   like a desktop export.

## Building it yourself

See [android/README.md](../android/README.md).
