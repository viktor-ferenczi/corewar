"""Publish artifacts from a successful CI run without executing PR code."""

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import subprocess
import tarfile
import tempfile
from pathlib import Path

VERSION = re.compile(r"v0\.1\.(0|[1-9][0-9]*)$")


def api(repo: str, endpoint: str, data: dict | None = None, *, pages: bool = False) -> dict | list:
    command = ["gh", "api", f"repos/{repo}/{endpoint}"]
    if data is not None:
        command.extend(["--method", "POST", "--input", "-"])
    if pages:
        command.extend(["--paginate", "--jq", ".[] | @json"])
    result = subprocess.run(
        command, input=json.dumps(data) if data is not None else None, capture_output=True, text=True, check=True
    )
    return [json.loads(line) for line in result.stdout.splitlines()] if pages else json.loads(result.stdout)


def eligible(run: dict, pull: dict | None = None) -> bool:
    if run.get("path") != ".github/workflows/ci.yml" or run.get("name") != "CI":
        return False
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        return False
    if run.get("event") == "push":
        return run.get("head_branch") == "main"
    return (
        run.get("event") == "pull_request"
        and pull is not None
        and pull.get("state") == "open"
        and not pull.get("draft", True)
        and pull["head"]["sha"] == run["head_sha"]
    )


def release_tag(releases: list[dict], tags: list[dict], run_id: int) -> tuple[str, dict | None]:
    marker = f"<!-- mars-ci-run:{run_id} -->"
    existing = next((release for release in releases if marker in (release.get("body") or "")), None)
    if existing is not None:
        if not VERSION.fullmatch(existing["tag_name"]):
            raise ValueError("CI release has an unexpected version")
        return existing["tag_name"], existing
    names = [release["tag_name"] for release in releases] + [tag["name"] for tag in tags]
    patches = [int(match[1]) for name in names if (match := VERSION.fullmatch(name))]
    return f"v0.1.{max(patches, default=-1) + 1}", None


def package(directory: Path, tag: str, run: dict) -> list[Path]:
    assets = []
    for feature in ["cpu", "gpu"]:
        binary = directory / f"mars-{feature}"
        if binary.is_symlink() or not binary.is_file() or binary.stat().st_size > 256 * 1024 * 1024:
            raise ValueError(f"Invalid build artifact: {binary.name}")
        code = binary.read_bytes()
        if not code.startswith(b"\x7fELF\x02\x01") or len(code) < 20 or code[18:20] != b"\x3e\x00":
            raise ValueError(f"Not a Linux x86-64 ELF binary: {binary.name}")
        metadata = (
            json.dumps(
                {"version": tag[1:], "commit": run["head_sha"], "run_id": run["id"], "features": feature}, indent=2
            ).encode()
            + b"\n"
        )
        archive = directory / f"mars-{tag}-linux-x86_64-{feature}.tar.gz"
        with archive.open("wb") as stream:
            with gzip.GzipFile(fileobj=stream, mode="wb", mtime=0, filename="") as compressed:
                with tarfile.open(fileobj=compressed, mode="w") as bundle:
                    for name, content, mode in [("mars", code, 0o755), ("BUILD.json", metadata, 0o644)]:
                        entry = tarfile.TarInfo(name)
                        entry.size = len(content)
                        entry.mode = mode
                        bundle.addfile(entry, io.BytesIO(content))
        assets.append(archive)
    sums = directory / "SHA256SUMS"
    sums.write_text("".join(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n" for path in assets))
    return assets + [sums]


def publish(repo: str, run_id: int, dry_run: bool) -> None:
    run = api(repo, f"actions/runs/{run_id}")
    pull = None
    if run.get("event") == "pull_request":
        candidates = api(repo, f"commits/{run['head_sha']}/pulls", pages=True)
        pull = next((candidate for candidate in candidates if eligible(run, candidate)), None)
    if not eligible(run, pull):
        print("No release: CI did not succeed on main or a current, ready-for-review PR")
        return
    if not re.fullmatch(r"[0-9a-f]{40}", run["head_sha"]):
        raise ValueError("Unexpected build commit")
    artifacts = api(repo, f"actions/runs/{run_id}/artifacts")["artifacts"]
    if not any(artifact["name"] == "mars-linux-x86_64" and not artifact["expired"] for artifact in artifacts):
        print("No release: this run has no retained build artifact")
        return
    releases = api(repo, "releases", pages=True)
    tags = api(repo, "tags", pages=True)
    tag, existing = release_tag(releases, tags, run_id)
    draft = run["event"] == "pull_request"
    if existing is not None and existing.get("target_commitish") != run["head_sha"]:
        raise ValueError("Existing release targets a different build")
    if existing is not None and not existing["draft"]:
        print(f"{tag} already published; no changes")
        return
    with tempfile.TemporaryDirectory(prefix="mars-release-") as temporary:
        directory = Path(temporary)
        subprocess.run(
            [
                "gh",
                "run",
                "download",
                str(run_id),
                "--repo",
                repo,
                "--name",
                "mars-linux-x86_64",
                "--dir",
                str(directory),
            ],
            check=True,
        )
        assets = package(directory, tag, run)
        label = f"PR #{pull['number']}" if draft else "main"
        notes = (
            f"Tested Linux x86-64 builds from {label}.\n\n"
            f"Commit: {run['head_sha']}\nCI: {run['html_url']}\n\n"
            "CPU-only and GPU-enabled archives include build metadata. Verify downloads with SHA256SUMS.\n"
            f"<!-- mars-ci-run:{run_id} -->"
        )
        if dry_run:
            print(f"Would {'draft' if draft else 'publish'} {tag}: {[path.name for path in assets]}")
            return
        release = existing or api(
            repo,
            "releases",
            {
                "tag_name": tag,
                "target_commitish": run["head_sha"],
                "name": f"MARS {tag} ({label})",
                "body": notes,
                "draft": True,
            },
        )
        subprocess.run(["gh", "release", "upload", tag, *map(str, assets), "--repo", repo, "--clobber"], check=True)
        if not draft:
            subprocess.run(
                [
                    "gh",
                    "api",
                    "--method",
                    "PATCH",
                    f"repos/{repo}/releases/{release['id']}",
                    "-F",
                    "draft=false",
                    "-F",
                    "make_latest=true",
                ],
                check=True,
                stdout=subprocess.DEVNULL,
            )
        print(f"{'Drafted' if draft else 'Published'} {tag}: {release['html_url']}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default=os.environ.get("GH_REPO", "viktor-ferenczi/corewar"))
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", args.repo) or args.run_id <= 0:
        parser.error("Invalid repository or run ID")
    publish(args.repo, args.run_id, args.dry_run)


if __name__ == "__main__":
    main()
