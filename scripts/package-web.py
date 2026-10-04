#!/usr/bin/env python3
"""Build a relocatable SmartAPC executable + locally served UI directory.

Development requires Cargo and Node >=22. Launching the package requires neither
Node nor Python; custom strategy compilation still requires a Rust toolchain.
An existing nonempty destination is rejected to protect unrelated files.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

REPO = Path(__file__).resolve().parents[1]


def run(command, directory):
    subprocess.run(command, cwd=directory, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=("debug", "release"), default="release")
    parser.add_argument("--output", type=Path, default=REPO / "dist" / "smartapc-web")
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error(f"destination is not empty: {output}; choose a new package directory")
    npm = "npm.cmd" if os.name == "nt" else "npm"
    run([npm, "ci"], REPO / "frontend")
    run([npm, "run", "build"], REPO / "frontend")
    cargo = ["cargo", "build", "--locked", "--bin", "smartapc"]
    if args.profile == "release":
        cargo.append("--release")
    run(cargo, REPO)
    executable = "smartapc.exe" if os.name == "nt" else "smartapc"
    target = Path(os.environ.get("CARGO_TARGET_DIR", str(REPO / "target")))
    if not target.is_absolute():
        target = REPO / target
    output.parent.mkdir(parents=True, exist_ok=True)
    # Assemble alongside the destination; an interrupted build never leaves a
    # half-built package at the documented launch path.
    with tempfile.TemporaryDirectory(prefix="smartapc-package-", dir=output.parent) as scratch:
        staged = Path(scratch) / "package"
        staged.mkdir()
        shutil.copy2(target / args.profile / executable, staged / executable)
        shutil.copytree(REPO / "frontend" / "dist", staged / "ui")
        (staged / "LAUNCH.md").write_text(
            "# SmartAPC browser workspace\n\n"
            f"From this directory run `{executable} serve --project-root ./projects` "
            "(prefix `./` on Unix). Open http://127.0.0.1:8080.\n\n"
            f"Create a project with `{executable} init-project --name demo --project-root ./projects`. "
            "Keep the executable and its `ui` directory together. You may launch "
            "from any working directory with an absolute `--project-root`.\n\n"
            "Virtual test plant: `smartapc virtual-hardware`; its browser URL is "
            "http://127.0.0.1:8081/virtual-hardware. Use `--no-web` for OPC UA only.\n\n"
            "Windows service commands are reference generation only; the service "
            "lifecycle has not been implemented or verified.\n",
            encoding="utf-8",
        )
        files = sorted(path for path in staged.rglob("*") if path.is_file())
        manifest = {path.relative_to(staged).as_posix(): hashlib.sha256(path.read_bytes()).hexdigest() for path in files}
        (staged / "package-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
        if output.exists():
            output.rmdir()  # Previously verified empty; never remove existing contents.
        staged.rename(output)
    print(f"Packaged SmartAPC at {output}")


if __name__ == "__main__":
    main()
