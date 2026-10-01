#!/usr/bin/env python3
"""Download the benchmark warriors named in field.json into warriors/, and log the download.

The warriors are other people's work. They stay out of Git (warriors/ is ignored), and every
download and use is recorded in USAGE.md. --repin rewrites field.json from the current archive;
review the diff before committing it.
"""

import argparse
import hashlib
import html
import io
import json
import re
import tarfile
import urllib.request
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIELD = HERE / "field.json"
WARRIORS = HERE / "warriors"
USAGE = HERE / "USAGE.md"
USER_AGENT = "corewar-breeding (https://github.com/viktor-ferenczi/corewar)"


def log_usage(run: str, purpose: str, warriors: str) -> None:
    """Append one line to USAGE.md: names and sources only, never code."""
    with USAGE.open("a") as f:
        f.write(f"| {date.today().isoformat()} | {run} | {purpose} | {warriors} |\n")


def load_field() -> dict:
    return json.loads(FIELD.read_text())


def field_summary(field: dict) -> str:
    return f"all {len(field['warriors'])} warriors of field.json ({field['source']}, {field['page']})"


def download(url: str) -> bytes:
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def members(archive: bytes) -> dict[str, bytes]:
    """The .red files of the archive by base name. Nothing is extracted to disk by tarfile."""
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        files = [m for m in tar if m.isfile() and m.name.endswith(".red") and not Path(m.name).name.startswith(".")]
        return {Path(m.name).name: tar.extractfile(m).read() for m in files}


def header(source: bytes, key: str) -> str:
    match = re.search(rf"^;{key}[ \t]+(.+?)\s*$", source.decode("latin-1"), re.MULTILINE | re.IGNORECASE)
    return match.group(1) if match else ""


def published_ranking(page: bytes) -> list[dict]:
    """Rank, name, author and score from the hill's table, in rank order."""
    text = html.unescape(re.sub(r"<[^>]*>", "", page.decode("latin-1")))
    rows = re.findall(r"^\s*(\d+)\s+(.+?)\s{2,}(.+?)\s+(\d+\.\d+) \(94\)\s*$", text, re.MULTILINE)
    return [
        {"rank": int(rank), "name": name, "author": author, "score": float(score)} for rank, name, author, score in rows
    ]


def repin(field: dict) -> None:
    archive = download(field["archive"])
    field["archive_sha256"] = hashlib.sha256(archive).hexdigest()
    field["pinned"] = date.today().isoformat()
    field["published"] = published_ranking(download(field["ranking"]))
    field["warriors"] = [
        {
            "file": name,
            "name": header(source, "name"),
            "author": header(source, "author"),
            "sha256": hashlib.sha256(source).hexdigest(),
        }
        for name, source in sorted(members(archive).items())
    ]
    FIELD.write_text(json.dumps(field, indent=1, ensure_ascii=False) + "\n")
    log_usage("fetch.py --repin", "pin the benchmark field", field_summary(field))
    print(f"pinned {len(field['warriors'])} warriors, review the diff of {FIELD.name}")


def fetch(field: dict) -> None:
    files = members(download(field["archive"]))
    wrong = [
        w["file"] for w in field["warriors"] if hashlib.sha256(files.get(w["file"], b"")).hexdigest() != w["sha256"]
    ]
    if wrong:
        raise SystemExit(
            f"missing or changed in the archive: {', '.join(wrong)}\n"
            "The hill has changed since it was pinned. Check the source's terms again, then run fetch.py --repin."
        )
    WARRIORS.mkdir(exist_ok=True)
    for warrior in field["warriors"]:
        (WARRIORS / warrior["file"]).write_bytes(files[warrior["file"]])
    log_usage("fetch.py", "download for the local breeding benchmark", field_summary(field))
    print(f"{len(field['warriors'])} warriors in {WARRIORS}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repin", action="store_true", help="rewrite field.json from the current archive")
    args = parser.parse_args()
    (repin if args.repin else fetch)(load_field())


if __name__ == "__main__":
    main()
