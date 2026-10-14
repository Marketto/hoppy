// Pure JVM module: protocol, discovery and transfer. No Android APIs, so it is
// unit-tested on the JVM and interop-tested against the Rust CLI (tools/interop.sh).
plugins {
    alias(libs.plugins.kotlin.jvm)
}

java {
    sourceCompatibility = JavaVersion.VERSION_17
    targetCompatibility = JavaVersion.VERSION_17
}

kotlin {
    // No jvmToolchain(17): Gradle cannot auto-provision a JDK 17 (no foojay resolver),
    // so on a JDK-21-only machine the build fails. Target 17 bytecode instead.
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

dependencies {
    testImplementation(libs.junit)
    testImplementation(libs.kotlin.test)
}

tasks.test {
    systemProperty("file.encoding", "UTF-8")
}

/** Classpath for tools/interop.sh: `gradle :core:printInteropClasspath -q`. */
tasks.register("printInteropClasspath") {
    dependsOn("testClasses")
    doLast {
        val cp = sourceSets["test"].runtimeClasspath.files.joinToString(File.pathSeparator)
        println(cp)
    }
}
