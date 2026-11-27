"""Generate the third-party notices from the desktop and Android Cargo lockfiles."""

import argparse
import os
import tomllib
from pathlib import Path
from urllib.parse import quote


ROOT = Path(__file__).resolve().parents[1]
FONT_NOTICES = (
    "Hack-Regular.txt",
    "OFL.txt",
    "UFL.txt",
    "emoji-icon-font-mit-license.txt",
)


def packages():
    locked = set()
    for filename in ("Cargo.lock", "android/Cargo.lock"):
        with (ROOT / filename).open("rb") as lockfile:
            locked.update(
                (package["name"], package["version"])
                for package in tomllib.load(lockfile)["package"]
                if package.get("source", "").startswith("registry+")
            )
    return sorted(locked)


def generate(cargo_homes):
    registries = [directory for home in cargo_homes for directory in (home / "registry/src").glob("*")]
    lines = [
        "# Third-party notices",
        "",
        "See [Credits and licenses](docs/credits.md) for the bundled data, fonts, Android build components, and adapted code.",
        "",
        "## Rust dependency licenses",
        "",
        "This inventory covers registry packages in the desktop and Android Cargo lockfiles, including build and target-specific dependencies.",
        "Each entry links to the exact upstream release and records its manifest license declaration.",
        "",
        "| Package | Declared license |",
        "| --- | --- |",
    ]
    missing = []
    font_licenses = []
    for name, version in packages():
        manifest = next(
            (path for directory in registries if (path := directory / f"{name}-{version}/Cargo.toml").is_file()),
            None,
        )
        if manifest is None:
            missing.append(f"{name} {version}")
            continue
        with manifest.open("rb") as source:
            package = tomllib.load(source)["package"]
        license_expression = package.get("license")
        if not license_expression and package.get("license-file"):
            license_expression = f"See upstream `{package['license-file']}`"
        if not license_expression:
            raise SystemExit(f"Review the upstream license for {name} {version}")
        url = f"https://crates.io/crates/{name}/{quote(version)}"
        lines.append(f"| [{name} {version}]({url}) | {license_expression} |")
        if name == "epaint_default_fonts":
            font_licenses.extend(
                (filename, (manifest.parent / "fonts" / filename).read_text(encoding="utf-8").strip())
                for filename in FONT_NOTICES
            )

    if missing:
        raise SystemExit("Missing Cargo sources. Run cargo fetch or supply another --cargo-home.\n" + "\n".join(missing))

    lines += ["", "## Bundled fonts", ""]
    for filename, notice in dict.fromkeys(font_licenses):
        lines += [f"### {filename}", "", "```text", notice, "```", ""]
    vendored = (ROOT / "android/vendor/egui_glow/LICENSE-MIT").read_text(encoding="utf-8").strip()
    lines += ["## Vendored egui_glow MIT notice", "", "```text", vendored, "```", ""]
    return "\n".join(lines)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cargo-home", action="append", type=Path, default=[])
    args = parser.parse_args()
    homes = [Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo")), *args.cargo_home]
    (ROOT / "THIRD-PARTY-NOTICES.md").write_text(generate(homes), encoding="utf-8", newline="\n")
