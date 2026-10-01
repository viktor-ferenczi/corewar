#!/usr/bin/env python3
"""REST API over the mars binary: a remote agent sends Redcode sources and gets tournament results.

  MARS_API_KEY=secret python3 serve.py [--host 127.0.0.1] [--port 8080] [--gpu LIST] [--max-wars N]

  GET  /             the limits and defaults of this server
  POST /compile      {"programs": {name: source}}
  POST /tournaments  {"programs": {name: source}, "against": {name: source}, "games": 100, ...}

Every request needs the pre-shared key: `Authorization: Bearer KEY`, or HTTP Basic with the key as
the password. The server speaks plain HTTP and is meant to sit behind a reverse proxy. See
README.md.
"""

import argparse
import base64
import hmac
import json
import os
import re
import secrets
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import breed

NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,63}")  # also a safe file name and never an option
STANDARDS = ("pmars", "hu93", "88", "94")
SETTINGS = ("steps", "core", "length", "distance", "queue")  # passed to mars, which checks them
MAX_BODY = 16 << 20
MAX_SOURCE = 64 << 10
MAX_PROGRAMS = 256  # of a full tournament; a gauntlet takes MAX_GAUNTLET in total
MAX_GAUNTLET = 16384
TIMEOUT = 600  # seconds for one mars call, far above what --max-wars allows


class Refused(Exception):
    """A request the server won't serve, with its HTTP status."""

    def __init__(self, status: int, message: str, **details):
        super().__init__(message)
        self.status = status
        self.body = {"error": message, **details}


def sources(body: dict, key: str) -> dict[str, str]:
    programs = body.get(key, {})
    if not isinstance(programs, dict):
        raise Refused(400, f"{key} must be an object of name: source")
    for name, source in programs.items():
        if not NAME.fullmatch(name):
            raise Refused(400, f"bad program name {name!r}: letters, digits, _ . - and at most 64 characters")
        if not isinstance(source, str) or len(source.encode()) > MAX_SOURCE:
            raise Refused(400, f"{name}: the source must be a string of at most {MAX_SOURCE} bytes")
    return programs


def rule_args(body: dict) -> list[str]:
    """The options that set the rules, for both `mars compile` and `mars tournament`."""
    standard = body.get("standard", "pmars")
    if standard not in STANDARDS:
        raise Refused(400, f"standard must be one of {', '.join(STANDARDS)}")
    args = ["--standard", standard]
    for setting in SETTINGS:
        if setting in body:
            if type(body[setting]) is not int or body[setting] < 0:
                raise Refused(400, f"{setting} must be a non-negative integer")
            args += [f"--{setting}", str(body[setting])]
    return args


def run_mars(*args: str) -> subprocess.CompletedProcess:
    try:
        done = subprocess.run([str(breed.MARS), *args], capture_output=True, text=True, timeout=TIMEOUT)
    except subprocess.TimeoutExpired:
        raise Refused(504, f"mars took longer than {TIMEOUT} seconds")
    if done.returncode == 2:  # the command line was refused: a setting out of range
        raise Refused(400, done.stderr.splitlines()[0].removeprefix("error: "))
    return done


def compile_programs(folder: Path, programs: dict[str, str], rules: list[str]) -> dict[str, dict]:
    """Write the sources into the folder and assemble them: the instructions or the errors by name."""
    if not programs:
        return {}
    for name, source in programs.items():
        (folder / name).write_text(source)
    lines = run_mars("compile", "--json", *rules, *(str(folder / name) for name in programs)).stdout.splitlines()
    compiled = {}
    for name, line in zip(programs, lines, strict=True):
        result = json.loads(line)
        del result["file"]
        if not result["ok"]:
            result["errors"] = [e.replace(str(folder / name), name) for e in result["errors"]]
        compiled[name] = result
    return compiled


def standings(runs: list[dict], names: list[str]) -> list[dict]:
    """Totals of the named programs over their runs, best score first."""
    rows = {name: {"name": name, "games": 0, "wins": 0, "draws": 0, "losses": 0} for name in names}
    for run in runs:
        for me, other in (("first", "second"), ("second", "first")):
            if row := rows.get(run[me]):
                row["games"] += run["games"]
                row["wins"] += run[f"{me}_wins"]
                row["draws"] += run["draws"]
                row["losses"] += run[f"{other}_wins"]
    for row in rows.values():
        row["score"] = round((row["wins"] + row["draws"] / 2) / row["games"], 4)
        row["points"] = round((3 * row["wins"] + row["draws"]) / row["games"] * 100, 1)  # as the hills score
    return sorted(rows.values(), key=lambda row: -row["score"])


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address: tuple[str, int], key: str, device: list[str], max_wars: int):
        super().__init__(address, Handler)
        self.key = key
        self.device = device
        self.max_wars = max_wars
        self.playing = threading.Lock()  # one tournament at a time has the GPUs

    def info(self) -> dict:
        return {
            "service": "corewar mars",
            "device": " ".join(self.device),
            "standards": STANDARDS,
            "settings": SETTINGS,
            "max_wars": self.max_wars,
            "max_programs": MAX_PROGRAMS,
            "max_gauntlet_programs": MAX_GAUNTLET,
            "max_source_bytes": MAX_SOURCE,
        }

    def compile(self, body: dict) -> dict:
        with tempfile.TemporaryDirectory(prefix="mars-serve-") as folder:
            return {"programs": compile_programs(Path(folder), sources(body, "programs"), rule_args(body))}

    def tournament(self, body: dict) -> dict:
        """A full tournament of the programs, or with `against` a gauntlet: every program plays
        every opponent. Programs that don't assemble are left out and reported."""
        candidates, opponents = sources(body, "programs"), sources(body, "against")
        if set(candidates) & set(opponents):
            raise Refused(400, "a name is in both programs and against")
        games = body.get("games", 100)
        seed = body.get("seed", secrets.randbits(48))
        if type(games) is not int or type(seed) is not int or games < 1 or seed < 0:
            raise Refused(400, "games must be a positive integer and seed a non-negative one")
        rules = rule_args(body)
        with tempfile.TemporaryDirectory(prefix="mars-serve-") as folder:
            folder = Path(folder)
            compiled = compile_programs(folder, candidates | opponents, rules)
            errors = {name: result["errors"] for name, result in compiled.items() if not result["ok"]}
            first = [name for name in candidates if name not in errors]
            second = [name for name in opponents if name not in errors]
            if opponents:
                pairs, limit = len(first) * len(second), MAX_GAUNTLET
            else:
                pairs, limit = len(first) * (len(first) - 1) // 2, MAX_PROGRAMS
            if pairs == 0:
                raise Refused(422, "nothing to play: fewer than two programs assemble", errors=errors)
            if len(first) + len(second) > limit:
                raise Refused(413, f"too many programs, at most {limit}")
            if pairs * games > self.max_wars:
                raise Refused(413, f"{pairs} pairs of {games} games are more than {self.max_wars} wars per request")
            out = folder / "runs.jsonl"
            against = ["--against", *(str(folder / name) for name in second)] if second else []
            files = [str(folder / name) for name in first]
            with self.playing:
                started = time.monotonic()
                played = run_mars(
                    *("tournament", "--games", str(games), "--seed", str(seed), "--out", str(out)),
                    *rules,
                    *self.device,
                    *files,
                    *against,
                )
                seconds = time.monotonic() - started
            if played.returncode != 0:
                raise Refused(422, played.stderr.strip().splitlines()[-1].removeprefix("error: "), errors=errors)
            runs = [json.loads(line) for line in out.read_text().splitlines()]
        return {
            "seed": seed,
            "games": games,
            "wars": pairs * games,
            "seconds": round(seconds, 2),
            "errors": errors,
            "standings": standings(runs, first),
            "runs": runs,
        }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server: Server

    def authorized(self) -> bool:
        scheme, _, value = self.headers.get("Authorization", "").partition(" ")
        if scheme.lower() == "basic":
            try:
                value = base64.b64decode(value).decode().partition(":")[2]
            except ValueError:
                return False
        elif scheme.lower() != "bearer":
            return False
        return hmac.compare_digest(value.strip().encode(), self.server.key.encode())

    def reply(self, status: int, body: dict) -> None:
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        if status == 401:
            self.send_header("WWW-Authenticate", 'Basic realm="mars"')
        self.end_headers()
        self.wfile.write(data)

    def handle_request(self, routes: dict) -> None:
        try:
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_BODY:
                self.close_connection = True  # the body stays unread
                raise Refused(413, f"the request body is larger than {MAX_BODY} bytes")
            raw = self.rfile.read(length)
            if not self.authorized():
                raise Refused(401, "missing or wrong key")
            route = routes.get(self.path.rstrip("/") or "/")
            if not route:
                raise Refused(404, "no such resource, see GET /")
            try:
                body = json.loads(raw) if raw else {}
            except ValueError:
                raise Refused(400, "the body is not JSON")
            if not isinstance(body, dict):
                raise Refused(400, "the body must be a JSON object")
            self.reply(200, route(body))
        except Refused as refused:
            self.reply(refused.status, refused.body)

    def do_GET(self) -> None:
        self.handle_request({"/": lambda body: self.server.info()})

    def do_POST(self) -> None:
        self.handle_request({"/compile": self.server.compile, "/tournaments": self.server.tournament})


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default="127.0.0.1", help="address to listen on (127.0.0.1)")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--gpu", help="discrete GPUs to use, like 1 or 0,1, or cpu (default: every discrete GPU)")
    parser.add_argument("--max-wars", type=int, default=2_000_000, help="wars per request (2000000)")
    args = parser.parse_args()
    key = os.environ.get("MARS_API_KEY", "")
    if len(key) < 16:
        raise SystemExit("set MARS_API_KEY to the pre-shared key, at least 16 characters")
    if not breed.MARS.exists():
        raise SystemExit(f"{breed.MARS} is missing, build it with: cargo build --release")
    server = Server((args.host, args.port), key, breed.device_args(args.gpu), args.max_wars)
    print(f"serving on http://{args.host}:{server.server_port} with mars {' '.join(server.device)}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
