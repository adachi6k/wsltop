#!/usr/bin/env python3
"""Generate WinGet manifests from a published wsltop release (stdlib + gh)."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile
import zipfile

PACKAGE = "Adachi6k.wsltop"
REPO = "adachi6k/wsltop"
SCHEMA = "1.12.0"


def version_from_tag(tag):
    if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("Expected a stable release tag vMAJOR.MINOR.PATCH")
    return tag[1:]


def verify_archive(archive, sidecar, tag):
    version_from_tag(tag)
    name = f"wsltop-{tag}-x86_64-pc-windows-msvc.zip"
    if archive.name != name:
        raise ValueError("Unexpected Windows archive name")
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if sidecar.read_text(encoding="ascii").strip() != f"{digest}  {name}":
        raise ValueError("Release checksum/sidecar format mismatch")
    nested = f"wsltop-{tag}-x86_64-pc-windows-msvc/wsltop.exe"
    with zipfile.ZipFile(archive) as zipped:
        files = [item.filename for item in zipped.infolist() if not item.is_dir()]
        expected = [nested, nested.rsplit('/', 1)[0] + '/README.md',
                    nested.rsplit('/', 1)[0] + '/LICENSE']
        if sorted(files) != sorted(expected):
            raise ValueError("Unexpected ZIP layout; review packaging before submission")
        with zipped.open(nested) as executable:
            if executable.read(2) != b"MZ":
                raise ValueError("Nested file is not a Windows executable")
    return digest, nested.replace("/", "\\")


def manifests(tag, digest, nested):
    version = version_from_tag(tag)
    common = f"PackageIdentifier: {PACKAGE}\nPackageVersion: {version}\n"
    url = f"https://github.com/{REPO}"
    content = {
        "version": common + "DefaultLocale: en-US\n",
        "installer": common + f"""InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: {nested}
  PortableCommandAlias: wsltop
MinimumOSVersion: 10.0.22000.0
UpgradeBehavior: uninstallPrevious
Commands:
- wsltop
Installers:
- Architecture: x64
  InstallerUrl: {url}/releases/download/{tag}/wsltop-{tag}-x86_64-pc-windows-msvc.zip
  InstallerSha256: {digest.upper()}
""",
        "defaultLocale": common + f"""PackageLocale: en-US
Publisher: Adachi6k
PublisherUrl: https://github.com/adachi6k
PublisherSupportUrl: {url}/issues
PackageName: wsltop
PackageUrl: {url}
License: MIT
LicenseUrl: {url}/blob/{tag}/LICENSE
ShortDescription: A unified top-like resource monitor for Windows, WSL2, Docker, and WSL Containers.
Description: |-
  Runs natively on Windows and WSL with interactive and one-shot views,
  CPU/memory/name sorting, resource hierarchy, and JSON output.
  Requires Windows 11 and a usable primary WSL2 distribution.
  Docker and WSL Containers are optional. WSL1 is not supported.
Moniker: wsltop
Tags:
- monitoring
- terminal
- wsl
- docker
ReleaseNotesUrl: {url}/releases/tag/{tag}
""",
    }
    names = {"version": f"{PACKAGE}.yaml", "installer": f"{PACKAGE}.installer.yaml",
             "defaultLocale": f"{PACKAGE}.locale.en-US.yaml"}
    return {names[kind]: f"# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kind}.{SCHEMA}.schema.json\n\n"
            + value + f"ManifestType: {kind}\nManifestVersion: {SCHEMA}\n"
            for kind, value in content.items()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = version_from_tag(args.tag)
    release = json.loads(subprocess.check_output([
        "gh", "release", "view", args.tag, "--repo", REPO,
        "--json", "tagName,isDraft,isPrerelease,assets"], text=True))
    if release["tagName"] != args.tag or release["isDraft"] or release["isPrerelease"]:
        raise ValueError("Only published stable releases may be submitted")
    name = f"wsltop-{args.tag}-x86_64-pc-windows-msvc.zip"
    expected_url = f"https://github.com/{REPO}/releases/download/{args.tag}/{name}"
    assets = {asset["name"]: asset["url"] for asset in release["assets"]}
    if assets.get(name) != expected_url or assets.get(name + '.sha256') != expected_url + '.sha256':
        raise ValueError("Expected official versioned ZIP and checksum assets")
    with tempfile.TemporaryDirectory() as temp:
        subprocess.run(["gh", "release", "download", args.tag, "--repo", REPO,
                        "--pattern", name, "--pattern", name + ".sha256", "--dir", temp], check=True)
        digest, nested = verify_archive(Path(temp) / name, Path(temp) / (name + '.sha256'), args.tag)
    directory = args.output / "manifests" / "a" / "Adachi6k" / "wsltop" / version
    directory.mkdir(parents=True, exist_ok=True)
    for name, content in manifests(args.tag, digest, nested).items():
        (directory / name).write_text(content, encoding="utf-8", newline="\n")
    print(directory)


if __name__ == "__main__":
    main()
