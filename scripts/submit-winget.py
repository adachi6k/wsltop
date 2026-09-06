#!/usr/bin/env python3
"""Submit already validated, freshly generated manifests using gh authentication."""
import argparse
import base64
import json
from pathlib import Path
import subprocess
from urllib.parse import urlencode

from winget import PACKAGE, manifests, version_from_tag


def api(path, data=None, missing_ok=False):
    command = ["gh", "api", path]
    if data is not None:
        command += ["--method", "POST", "--input", "-"]
    result = subprocess.run(command, input=json.dumps(data) if data is not None else None,
                            text=True, capture_output=True)
    if result.returncode:
        if missing_ok and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)


def submit(directory, tag):
    version = version_from_tag(tag)
    prefix = f"manifests/a/Adachi6k/wsltop/{version}"
    expected = set(manifests(tag, '0' * 64, 'placeholder'))
    if {p.name for p in directory.iterdir()} != expected:
        raise ValueError("Submit exactly the three generated manifest files")
    files = {p.name: p.read_text(encoding="utf-8") for p in directory.iterdir()}
    for content in files.values():
        if f"PackageIdentifier: {PACKAGE}\n" not in content or f"PackageVersion: {version}\n" not in content:
            raise ValueError("Manifest identity/version mismatch")
    upstream = "repos/microsoft/winget-pkgs"
    if api(f"{upstream}/contents/{prefix}", missing_ok=True) is not None:
        print(f"{PACKAGE} {version} already exists upstream; no submission needed")
        return
    # Also catch submissions from other publishers/bots before creating a fork.
    query = urlencode({"q": f"repo:microsoft/winget-pkgs is:pr is:open {PACKAGE} {version} in:title"})
    existing = api("search/issues?" + query)["items"]
    if existing:
        print("Existing submission: " + existing[0]["html_url"])
        return
    owner = api("user")["login"]
    fork_path = f"repos/{owner}/winget-pkgs"
    fork = api(fork_path, missing_ok=True)
    if fork is None:
        api(upstream + "/forks", {"default_branch_only": True})
        fork = api(fork_path)
    if not fork.get("fork") or fork.get("parent", {}).get("full_name") != "microsoft/winget-pkgs":
        raise ValueError("Expected the authenticated user's winget-pkgs fork")
    branch = f"wsltop-{version}"
    ref = api(f"{fork_path}/git/ref/heads/{branch}", missing_ok=True)
    if ref is not None:
        # Reruns recover a branch created before PR creation without overwriting it.
        for name, content in files.items():
            remote = api(f"{fork_path}/contents/{prefix}/{name}?" + urlencode({"ref": branch}))
            if base64.b64decode(remote["content"]).decode() != content:
                raise ValueError("Existing submission branch differs; review it manually")
    else:
        base = api(upstream + "/git/ref/heads/master")["object"]["sha"]
        base_tree = api(upstream + "/git/commits/" + base)["tree"]["sha"]
        tree = api(fork_path + "/git/trees", {
            "base_tree": base_tree,
            "tree": [{"path": f"{prefix}/{name}", "mode": "100644", "type": "blob", "content": content}
                     for name, content in sorted(files.items())],
        })
        commit = api(fork_path + "/git/commits", {
            "message": f"Add {PACKAGE} {version}", "tree": tree["sha"], "parents": [base],
        })
        api(fork_path + "/git/refs", {"ref": "refs/heads/" + branch, "sha": commit["sha"]})
    comparison = api(f"{upstream}/compare/master...{owner}:{branch}")
    changes = comparison.get("files", [])
    if ({item["filename"] for item in changes} != {f"{prefix}/{name}" for name in files}
            or any(item["status"] != "added" for item in changes)):
        raise ValueError("Submission branch must add only this version's three manifests")
    body = f"""## Description

Add {PACKAGE} {version}, a portable Windows/WSL resource monitor.
Official release: https://github.com/adachi6k/wsltop/releases/tag/{tag}
The ZIP checksum and nested executable path were verified before manifest generation.
Windows 11 and a usable WSL2 distribution are required to monitor workloads.

## Manifest checklist

- [x] Checked for existing submissions for this package version.
- [x] Only one package version and its three manifest files are included.
- [x] Validated with `winget validate --manifest`.
- [x] Tested local manifest installation and installed executable version/help.
- [x] Uses manifest schema 1.10.0.
- [ ] Contributor License Agreement, if required by the Microsoft CLA bot.
"""
    is_new = api(f"{upstream}/contents/manifests/a/Adachi6k/wsltop", missing_ok=True) is None
    result = api(upstream + "/pulls", {
        "title": f"{'New package' if is_new else 'New version'}: {PACKAGE} version {version}",
        "head": f"{owner}:{branch}", "base": "master", "body": body,
    })
    print(result["html_url"])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--manifest-dir", required=True, type=Path)
    args = parser.parse_args()
    submit(args.manifest_dir, args.tag)
