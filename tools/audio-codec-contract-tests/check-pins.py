"""Ensure the headless codec seam uses the desktop's actual codec dependencies."""
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parents[2]
HARNESS = Path(__file__).resolve().parent


def codec_pins(path):
    packages = tomllib.loads(path.read_text())["package"]
    return {
        item["name"]: (item["version"], item.get("source"))
        for item in packages
        if item["name"].startswith("symphonia") or item["name"] == "ffmpeg-sidecar"
    }


desktop = tomllib.loads((ROOT / "frontend/src-tauri/Cargo.toml").read_text())
harness = tomllib.loads((HARNESS / "Cargo.toml").read_text())
assert sorted(desktop["dependencies"]["symphonia"]["features"]) == sorted(
    harness["dependencies"]["symphonia"]["features"]
), "Codec features differ from production"
assert codec_pins(ROOT / "Cargo.lock") == codec_pins(HARNESS / "Cargo.lock"), (
    "Codec versions or FFmpeg discovery source differ from production"
)
print("Production codec dependency parity PASS")
