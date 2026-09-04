import java.io.File
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.ksp)
}

val keystorePropertiesFile = file(
    providers.environmentVariable("LAM_ANDROID_KEYSTORE_PROPERTIES").orNull
        ?: "${System.getProperty("user.home")}/.config/lam/android-signing.properties",
)
val keystoreProperties = Properties().apply {
    if (keystorePropertiesFile.isFile) {
        keystorePropertiesFile.inputStream().use(::load)
    }
}
val signingPropertyNames = listOf("storeFile", "storePassword", "keyAlias", "keyPassword")
val releaseSigningConfigured =
    keystorePropertiesFile.isFile && signingPropertyNames.all { !keystoreProperties.getProperty(it).isNullOrBlank() }

android {
    namespace = "dev.carraes.lam"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.carraes.lam"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    signingConfigs {
        if (releaseSigningConfigured) {
            create("release") {
                storeFile = rootProject.file(keystoreProperties.getProperty("storeFile"))
                storePassword = keystoreProperties.getProperty("storePassword")
                keyAlias = keystoreProperties.getProperty("keyAlias")
                keyPassword = keystoreProperties.getProperty("keyPassword")
                storeType = "PKCS12"
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            if (releaseSigningConfigured) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    lint {
        warningsAsErrors = true
        disable += setOf(
            "AndroidGradlePluginVersion",
            "GradleDependency",
            "MissingApplicationIcon",
            "NewerVersionAvailable",
            "OldTargetApi",
        )
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

ksp {
    arg("room.schemaLocation", "$projectDir/schemas")
}

val verifyReleaseSigning = tasks.register("verifyReleaseSigning") {
    group = "verification"
    description = "Fails release builds unless the private signing key is configured."
    inputs.property("signingPropertiesPath", keystorePropertiesFile.absolutePath)

    doLast {
        val signingFile = File(inputs.properties.getValue("signingPropertiesPath") as String)
        if (!signingFile.isFile) {
            throw GradleException(
                "Release signing requires $signingFile. Use the existing private key; never generate a replacement during an upgrade.",
            )
        }

        val signingProperties = Properties().apply {
            signingFile.inputStream().use(::load)
        }
        val requiredNames = listOf("storeFile", "storePassword", "keyAlias", "keyPassword")
        val missing = requiredNames.filter { signingProperties.getProperty(it).isNullOrBlank() }
        if (missing.isNotEmpty()) {
            throw GradleException("Release signing properties are missing: ${missing.joinToString()}.")
        }

        val keyFile = File(signingProperties.getProperty("storeFile"))
        if (!keyFile.isFile) {
            throw GradleException(
                "Release key is missing at ${signingProperties.getProperty("storeFile")}. Stop instead of generating a replacement key.",
            )
        }
    }
}

val verifyCompileSdk36Dependencies = tasks.register("verifyCompileSdk36Dependencies") {
    group = "verification"
    description = "Checks that debug dependencies remain compatible with compile SDK 36."
    dependsOn("checkDebugAarMetadata", "checkDebugAndroidTestAarMetadata")
}

tasks.named("check").configure {
    dependsOn(verifyCompileSdk36Dependencies)
}

tasks.matching { it.name == "preReleaseBuild" }.configureEach {
    dependsOn(verifyReleaseSigning)
}

dependencies {
    implementation(platform(libs.compose.bom))
    androidTestImplementation(platform(libs.compose.bom))

    implementation(libs.core.ktx)
    implementation(libs.activity.compose)
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.lifecycle.runtime.ktx)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.navigation.compose)
    implementation(libs.room.runtime)
    implementation(libs.room.ktx)
    ksp(libs.room.compiler)
    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.camerax.camera2)
    implementation(libs.camerax.lifecycle)
    implementation(libs.camerax.view)
    implementation(libs.mlkit.barcode)
    implementation(libs.browser)
    implementation(libs.markdown.core)
    implementation(libs.markdown.material3)

    debugImplementation(libs.compose.ui.tooling)
    debugImplementation(libs.compose.ui.test.manifest)
    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
    testImplementation(libs.kotlinx.coroutines.test)
    androidTestImplementation(libs.androidx.test.ext.junit)
    androidTestImplementation(libs.espresso.core)
    androidTestImplementation(libs.compose.ui.test.junit4)
}
