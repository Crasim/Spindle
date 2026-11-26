"""Package the static Spindle downloads page for an ordinary web host."""

import argparse
from pathlib import Path
import tomllib
from zipfile import ZIP_DEFLATED, ZipFile


ROOT = Path(__file__).resolve().parents[1]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="Directory for the rendered page and ZIP")
    args = parser.parse_args()
    args.destination.mkdir(parents=True, exist_ok=True)
    with (ROOT / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["package"]["version"]
    page = (ROOT / "website/index.html").read_text(encoding="utf-8")
    page = page.replace("v@RELEASE_VERSION@", f"v{version}")
    page = page.replace("@RELEASE_LABEL@", version.replace("-beta.", " beta "))
    files = {
        "index.html": page.encode("utf-8"),
        "spindle-256.png": (ROOT / "assets/spindle-256.png").read_bytes(),
    }
    with ZipFile(args.destination / "spindle-website.zip", "w", ZIP_DEFLATED) as archive:
        for name, data in files.items():
            (args.destination / name).write_bytes(data)
            archive.writestr(name, data)


if __name__ == "__main__":
    main()
