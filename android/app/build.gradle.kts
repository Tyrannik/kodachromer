plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The app's version follows the Rust workspace (one source of truth).
val workspaceVersion: String = run {
    val toml = rootProject.file("../Cargo.toml").readText()
    Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(toml)!!.groupValues[1]
}

// versionCode must grow with every release: taken from CI's run number,
// so each CI-built APK installs over the previous one.
val buildNumber: Int = (System.getenv("GITHUB_RUN_NUMBER") ?: "1").toInt()

android {
    namespace = "io.github.tyrannik.remjet"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.tyrannik.remjet"
        minSdk = 24
        targetSdk = 35
        versionCode = buildNumber
        versionName = workspaceVersion
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
    }

    signingConfigs {
        // A key committed to the repo, so every build (CI or local) has the
        // same signature and installs over the last one without uninstalling.
        // It only proves "built from this repo", which is all a sideloaded
        // app needs. A Play Store release would use a private key instead.
        create("sideload") {
            storeFile = file("remjet-sideload.keystore")
            storePassword = "remjet-sideload"
            keyAlias = "remjet"
            keyPassword = "remjet-sideload"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("sideload")
        }
        debug {
            signingConfig = signingConfigs.getByName("sideload")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs {
            // Keep the .so uncompressed and page-aligned (faster start).
            useLegacyPackaging = false
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
