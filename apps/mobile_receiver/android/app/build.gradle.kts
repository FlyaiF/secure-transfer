import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

val rustAndroidTargets =
    listOf(
        mapOf(
            "abi" to "arm64-v8a",
            "target" to "aarch64-linux-android",
            "linkerEnv" to "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
            "linker" to "aarch64-linux-android21-clang",
        ),
        mapOf(
            "abi" to "armeabi-v7a",
            "target" to "armv7-linux-androideabi",
            "linkerEnv" to "CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER",
            "linker" to "armv7a-linux-androideabi21-clang",
        ),
        mapOf(
            "abi" to "x86_64",
            "target" to "x86_64-linux-android",
            "linkerEnv" to "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER",
            "linker" to "x86_64-linux-android21-clang",
        ),
    )
val rustNdkVersion = "28.2.13676358"

android {
    namespace = "dev.visualtransfer.mobile_receiver"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = rustNdkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        // TODO: Specify your own unique Application ID (https://developer.android.com/studio/build/application-id.html).
        applicationId = "dev.visualtransfer.mobile_receiver"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir(layout.buildDirectory.get().dir("rustJniLibs").asFile)
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

fun androidSdkDir(): File {
    val properties = Properties()
    val localProperties = rootProject.file("local.properties")
    if (localProperties.exists()) {
        localProperties.inputStream().use { properties.load(it) }
    }
    val sdkDir =
        properties.getProperty("sdk.dir")
            ?: System.getenv("ANDROID_HOME")
            ?: System.getenv("ANDROID_SDK_ROOT")
            ?: error("Android SDK path not found")
    return file(sdkDir)
}

fun ndkHostTag(): String {
    val os = System.getProperty("os.name").lowercase()
    val arch = System.getProperty("os.arch").lowercase()
    return when {
        os.contains("mac") -> "darwin-x86_64"
        os.contains("linux") -> "linux-x86_64"
        os.contains("windows") -> "windows-x86_64"
        else -> error("Unsupported NDK host OS: $os/$arch")
    }
}

val repoRoot = rootProject.projectDir.resolve("../../..").canonicalFile
val jniLibsDir = layout.buildDirectory.dir("rustJniLibs")

tasks.register("buildRustAndroid") {
    val mobileCoreDir = repoRoot.resolve("crates/mobile-core")
    inputs.file(repoRoot.resolve("Cargo.lock"))
    inputs.file(repoRoot.resolve("Cargo.toml"))
    inputs.file(mobileCoreDir.resolve("Cargo.toml"))
    inputs.dir(mobileCoreDir.resolve("src"))
    outputs.dir(jniLibsDir)

    doLast {
        val sdkDir = androidSdkDir()
        val ndkDir = sdkDir.resolve("ndk/$rustNdkVersion").takeIf { it.exists() }
            ?: sdkDir.resolve("ndk-bundle").takeIf { it.exists() }
            ?: error("Android NDK $rustNdkVersion not found under $sdkDir")
        val llvmBin = ndkDir.resolve("toolchains/llvm/prebuilt/${ndkHostTag()}/bin")
        val llvmAr = llvmBin.resolve("llvm-ar")

        rustAndroidTargets.forEach { target ->
            val rustTarget = target.getValue("target")
            val linker = llvmBin.resolve(target.getValue("linker"))
            val cargo =
                ProcessBuilder(
                    "cargo",
                    "build",
                    "--package",
                    "transfer-mobile-core",
                    "--release",
                    "--target",
                    rustTarget,
                )
                    .directory(repoRoot)
                    .redirectOutput(ProcessBuilder.Redirect.INHERIT)
                    .redirectError(ProcessBuilder.Redirect.INHERIT)

            cargo.environment()[target.getValue("linkerEnv")] = linker.absolutePath
            cargo.environment()["AR_${rustTarget.replace("-", "_")}"] = llvmAr.absolutePath

            val exitCode = cargo.start().waitFor()
            if (exitCode != 0) {
                error("cargo build failed for $rustTarget with exit code $exitCode")
            }

            val builtLibrary =
                repoRoot.resolve("target/$rustTarget/release/libtransfer_mobile_core.so")
            require(builtLibrary.exists()) {
                "Rust build did not produce ${builtLibrary.absolutePath}"
            }
            copy {
                from(builtLibrary)
                into(jniLibsDir.get().dir(target.getValue("abi")).asFile)
            }
        }
    }
}

tasks.named("preBuild") {
    dependsOn("buildRustAndroid")
}
