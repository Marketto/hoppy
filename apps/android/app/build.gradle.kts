import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "app.hoppy"
    compileSdk = 35

    defaultConfig {
        applicationId = "app.hoppy"
        minSdk = 29 // Android 10: scoped storage + MediaStore.Downloads
        targetSdk = 35
        // The release workflow passes -PhoppyVersionName=1.2.3 -PhoppyVersionCode=<run number>.
        versionCode = (project.findProperty("hoppyVersionCode") as String?)?.toInt() ?: 1
        versionName = (project.findProperty("hoppyVersionName") as String?) ?: "0.1.0-dev"
    }

    // Release signing comes ONLY from CI secrets (docs/release.md); nothing is committed.
    // Without them (local builds, forks) release falls back to the debug key.
    val keystoreFile = System.getenv("HOPPY_KEYSTORE_FILE")
    signingConfigs {
        if (keystoreFile != null) {
            create("release") {
                storeFile = file(keystoreFile)
                storePassword = System.getenv("HOPPY_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("HOPPY_KEY_ALIAS")
                keyPassword = System.getenv("HOPPY_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        viewBinding = true
    }
    packaging {
        resources {
            // BouncyCastle (via :core) ships OSGi/multi-release metadata the APK does not need.
            excludes += "META-INF/versions/9/OSGI-INF/MANIFEST.MF"
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    implementation(project(":core"))
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.appcompat)
    implementation(libs.androidx.activity.ktx)
    implementation(libs.androidx.recyclerview)
    implementation(libs.material)
    // In-app QR scanner + QR rendering for device verification (docs/protocol.md §7.7).
    implementation(libs.zxing.android.embedded)

    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test)
}
