#!/usr/bin/env python3
"""Build a dependency-free Java/XML Android app with an installed toolchain."""

import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile
from zipfile import ZIP_DEFLATED, ZipFile


def run(*args):
    subprocess.run(args, check=True)


def build(project, android_jar, min_api, output, keystore):
    manifest = project / "AndroidManifest.xml"
    sources = sorted((project / "src").rglob("*.java"))
    resources = project / "res"
    if not manifest.is_file() or not sources or not android_jar.is_file():
        raise ValueError("Provide AndroidManifest.xml, src/**/*.java, and a local Android platform jar.")
    required = ("javac", "d8", "aapt2", "zipalign", "apksigner", "keytool")
    missing = [tool for tool in required if not shutil.which(tool)]
    if missing:
        raise ValueError(f"Missing from PATH: {', '.join(missing)}")
    if output.suffix.lower() != ".apk" or output == keystore:
        raise ValueError("Choose an APK output path separate from the debug keystore.")

    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".spindle-apk-", dir=output.parent) as staging:
        work = Path(staging)
        classes = work / "classes"
        classes.mkdir()
        run("javac", "-proc:none", "-encoding", "UTF-8", "-source", "8", "-target", "8",
            "-bootclasspath", android_jar, "-d", classes, *sources)

        dex = work / "dex"
        dex.mkdir()
        run("d8", "--min-api", str(min_api), "--lib", android_jar, "--output", dex,
            *sorted(classes.rglob("*.class")))

        base = work / "base.apk"
        link = ["aapt2", "link", "-o", base, "-I", android_jar, "--manifest", manifest]
        if resources.is_dir() and any(path.is_file() for path in resources.rglob("*")):
            compiled = work / "resources.zip"
            run("aapt2", "compile", "--dir", resources, "-o", compiled)
            link.append(compiled)
        run(*link)
        with ZipFile(base, "a", ZIP_DEFLATED) as archive:
            for path in sorted(dex.glob("classes*.dex")):
                archive.write(path, path.name)

        aligned = work / "aligned.apk"
        run("zipalign", "-f", "-p", "4", base, aligned)
        if not keystore.exists():
            keystore.parent.mkdir(parents=True, exist_ok=True)
            run("keytool", "-genkeypair", "-keystore", keystore, "-storepass", "android",
                "-keypass", "android", "-alias", "androiddebugkey", "-keyalg", "RSA",
                "-keysize", "2048", "-validity", "10000", "-dname", "CN=Android Debug")
        signed = work / "signed.apk"
        run("apksigner", "sign", "--ks", keystore, "--ks-key-alias", "androiddebugkey",
            "--ks-pass", "pass:android", "--key-pass", "pass:android", "--out", signed, aligned)
        run("apksigner", "verify", "--verbose", signed)
        with ZipFile(signed) as archive:
            if "classes.dex" not in archive.namelist() or archive.testzip() is not None:
                raise ValueError("The APK failed its archive check.")
        signed.replace(output)
    print(f"Built and verified {output}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("project", type=Path, help="Directory containing AndroidManifest.xml and src/")
    parser.add_argument("--android-jar", required=True, type=Path, help="Installed Android platform jar")
    parser.add_argument("--min-api", required=True, type=int, help="Minimum API from AndroidManifest.xml")
    parser.add_argument("--output", required=True, type=Path, help="Path for the verified debug APK")
    parser.add_argument("--keystore", type=Path, default=Path.home() / ".android/debug.keystore",
                        help="Local Android debug keystore")
    args = parser.parse_args()
    try:
        build(args.project.resolve(), args.android_jar.resolve(), args.min_api,
              args.output.absolute(), args.keystore.resolve())
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Build failed: {error}\n")


if __name__ == "__main__":
    main()
