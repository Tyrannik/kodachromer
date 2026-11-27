# Android app

A plain Android app (Kotlin, framework widgets only, no AndroidX) that calls
`remjet-core` through the JNI bindings in [`crates/remjet-android`](../crates/remjet-android).
Install and use: [docs/ANDROID.md](../docs/ANDROID.md).

CI (`.github/workflows/android.yml`) builds the APK on every change to the app or the Rust code
it uses. To build locally you need the Android SDK and NDK (Android Studio installs both), JDK 17
and `cargo install cargo-ndk`, then from the repository root:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo ndk --platform 24 -t arm64-v8a -t armeabi-v7a -o android/app/src/main/jniLibs build --release -p remjet-android
cd android
./gradlew assembleRelease     # gradlew.bat on Windows
```

The APK is `android/app/build/outputs/apk/release/app-release.apk`.

To try the Android code path on the desktop (timing, peak memory, output) without a phone:

```sh
cargo run --release -p remjet-android --example android_render -- photo.dng portra400 out half
```

## Signing

`app/remjet-sideload.keystore` is committed on purpose, with its password in `build.gradle.kts`.
Every build is signed with it, so a new APK installs over the previous one. It identifies "built
from this repository" and nothing more, which is all sideloading needs. Publishing to the Play
Store would need a private upload key kept out of the repository.
