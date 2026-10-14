import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// Release signing key: android/key.properties (gitignored, never committed), see mobile/README.md:
//   storeFile=/absolute/path/to/dialog-release.jks
//   storePassword=...
//   keyAlias=dialog
//   keyPassword=...
val keystoreProperties = Properties().apply {
    val f = rootProject.file("key.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
val hasReleaseKey = listOf("storeFile", "storePassword", "keyAlias", "keyPassword")
    .all { !keystoreProperties.getProperty(it).isNullOrBlank() }

android {
    namespace = "pm.dialog.chat"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
        // Required by flutter_local_notifications.
        isCoreLibraryDesugaringEnabled = true
    }

    defaultConfig {
        applicationId = "pm.dialog.chat"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = maxOf(24, flutter.minSdkVersion)
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        // You can force using the value of versionCode by specifying the `-P force-version-code-ignoring-abi=true`
        // flag during build.
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        if (hasReleaseKey) {
            create("release") {
                storeFile = file(keystoreProperties.getProperty("storeFile"))
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            // Never signed with the debug key: without android/key.properties the release
            // build stops (see the check below).
            if (hasReleaseKey) signingConfig = signingConfigs.getByName("release")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
    // VoiceFx.kt plugs into flutter_webrtc's audio processing, whose types come from the WebRTC
    // SDK: the same version as flutter_webrtc's (pub cache, android/build.gradle), compile only
    // (the plugin ships it).
    compileOnly("io.github.webrtc-sdk:android:150.7871.01")
}

// Push notifications are optional: the Google services plugin is applied only when a Firebase
// configuration (android/app/google-services.json) is present. See mobile/README.md.
if (file("google-services.json").exists()) {
    apply(plugin = "com.google.gms.google-services")
}

// A release build without the release key fails with an explanation instead of producing an
// unsigned (or debug-signed) package. Debug and profile builds are not affected.
gradle.taskGraph.whenReady {
    val releaseTasks = allTasks.filter { t ->
        t.project == project && (t.name.startsWith("assembleRelease") || t.name.startsWith("bundleRelease") ||
            t.name.startsWith("packageRelease") || t.name.startsWith("signRelease"))
    }
    if (releaseTasks.isNotEmpty() && !hasReleaseKey) {
        throw GradleException(
            "Release signing is not configured: create android/key.properties with storeFile, " +
                "storePassword, keyAlias and keyPassword (see mobile/README.md, \"Android release signing\").",
        )
    }
}
