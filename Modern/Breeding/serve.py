#!/usr/bin/env python3
"""REST API over the mars binary: a remote agent sends Redcode sources and gets tournament results.

  MARS_API_KEY=secret python3 serve.py [--host 127.0.0.1] [--port 8080] [--gpu LIST] [--max-wars N]

  GET  /             the limits and defaults of this server
  GET  /livez        cheap: the process answers (no key needed)
  GET  /health       extensive: assembles and plays a tiny tournament on the configured device
  POST /compile      {"programs": {id: source}}
  POST /tournaments  {"programs": {id: source}, "against": {id: source}, "games": 100, ...}
                     queues a tournament and answers with its id
  GET  /tournaments           the queued, running and recently finished tournaments
  GET  /tournaments/ID        its status and progress, and the results once it is done
  DELETE /tournaments/ID      cancel it, or forget a finished one

?wait=SECONDS on the POST and on GET /tournaments/ID holds the answer until the tournament is over
or the time is up (long poll).

Every request but /livez needs the pre-shared key: `Authorization: Bearer KEY`, or HTTP Basic with the key as
the password. The server speaks plain HTTP and is meant to sit behind a reverse proxy. See
README.md.
"""

import argparse
import base64
import hmac
import json
import os
import re
import queue
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
import uuid
from collections import deque
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

import breed

MAX_ID = 200  # characters of a program ID, a free string: a name, a UUID, a hash
STANDARDS = ("pmars", "hu93", "88", "94")
SETTINGS = ("steps", "core", "length", "distance", "queue")  # passed to mars, which checks them
MAX_BODY = 16 << 20
MAX_SOURCE = 64 << 10
MAX_PROGRAMS = 256  # of a full tournament; a gauntlet takes MAX_GAUNTLET in total
MAX_GAUNTLET = 16384
TIMEOUT = 60  # seconds for assembling or the health check's wars
MAX_WAIT = 300  # seconds a long poll may hold a request
MAX_LABEL = 200
OVER = ("done", "failed", "canceled")


class Refused(Exception):
    """A request the server won't serve, with its HTTP status."""

    def __init__(self, status: int, message: str, /, **details):
        super().__init__(message)
        self.status = status
        self.body = {"error": message, **details}


def sources(body: dict, key: str) -> dict[str, str]:
    """The programs of a request by their IDs."""
    programs = body.get(key, {})
    if not isinstance(programs, dict):
        raise Refused(400, f"{key} must be an object of id: source")
    for program_id, source in programs.items():
        if not 0 < len(program_id) <= MAX_ID:
            raise Refused(400, f"a program ID must have 1 to {MAX_ID} characters")
        if not isinstance(source, str) or len(source.encode()) > MAX_SOURCE:
            raise Refused(400, f"{program_id}: the source must be a string of at most {MAX_SOURCE} bytes")
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


def compile_programs(folder: Path, programs: dict[str, str], rules: list[str]) -> tuple[dict[str, dict], dict]:
    """Write the sources into the folder and assemble them.

    Returns the instructions or the errors by program ID, and the files by ID. The files are
    numbered, so an ID is free text and never reaches a path or the command line.
    """
    files = {program_id: folder / f"p{n}" for n, program_id in enumerate(programs)}
    if not programs:
        return {}, files
    for program_id, source in programs.items():
        files[program_id].write_text(source)
    lines = run_mars("compile", "--json", *rules, *map(str, files.values())).stdout.splitlines()
    compiled = {}
    for program_id, line in zip(programs, lines, strict=True):
        result = json.loads(line)
        del result["file"]
        if not result["ok"]:
            result["errors"] = [e.replace(str(files[program_id]), program_id) for e in result["errors"]]
        compiled[program_id] = result
    return compiled, files


def standings(runs: list[dict], names: list[str]) -> list[dict]:
    """Totals of the named programs over their runs, best score first."""
    rows = {name: {"id": name, "games": 0, "wins": 0, "draws": 0, "losses": 0} for name in names}
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


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


class Job:
    """A submitted tournament. The server's lock guards its status, process and results."""

    def __init__(
        self,
        label: str,
        seed: int,
        games: int,
        wars: int,
        programs: dict[str, str],
        against: dict[str, str],
        errors: dict,
        files: dict[str, Path],
        args: list[str],
    ):
        self.id = str(uuid.uuid4())
        self.label = label
        self.programs = programs  # as sent, returned with the results
        self.against = against
        self.ids = {path.name: program_id for program_id, path in files.items()}  # by the name mars reports
        self.seed = seed
        self.games = games
        self.wars = wars
        self.errors = errors  # programs that did not assemble
        self.args = args
        self.folder: Path | None = None
        self.status = "queued"
        self.wars_done = 0
        self.created = now()
        self.started: str | None = None
        self.finished: str | None = None
        self.clock = 0.0  # monotonic time of the last status change, for seconds and pruning
        self.seconds: float | None = None
        self.error: str | None = None
        self.runs: list[dict] = []
        self.process: subprocess.Popen | None = None
        self.over = threading.Event()

    def view(self, results: bool) -> dict:
        view = {
            "id": self.id,
            "label": self.label,
            "status": self.status,
            "progress": round(100 * self.wars_done / self.wars, 1),
            "wars_done": self.wars_done,
            "wars": self.wars,
            "games": self.games,
            "seed": self.seed,
            "created": self.created,
            "started": self.started,
            "finished": self.finished,
            "errors": self.errors,
        }
        if self.status == "failed":
            view["error"] = self.error
        if self.status == "done":
            view["seconds"] = self.seconds
            if results:
                played = [program_id for program_id in self.programs if program_id not in self.errors]
                view |= {"standings": standings(self.runs, played), "runs": self.runs}
        if results:
            view |= {"programs": self.programs, "against": self.against}
        return view


class Server(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(
        self,
        address: tuple[str, int],
        key: str,
        device: list[str],
        max_wars: int,
        max_queued: int = 10,
        job_timeout: float = 3600,
        keep: float = 3600,
    ):
        super().__init__(address, Handler)
        self.key = key
        self.device = device
        self.max_wars = max_wars
        self.max_queued = max_queued
        self.job_timeout = job_timeout
        self.keep = keep  # seconds a finished tournament stays listed
        self.jobs: dict[str, Job] = {}  # in submission order
        self.lock = threading.Lock()
        self.queue: queue.Queue[Job] = queue.Queue()
        threading.Thread(target=self.work, daemon=True).start()  # one tournament at a time has the GPUs

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
            "max_queued": self.max_queued,
            "max_id_characters": MAX_ID,
            "max_wait_seconds": MAX_WAIT,
            "tournament_timeout_seconds": self.job_timeout,
            "finished_kept_seconds": self.keep,
        }

    def health(self) -> dict:
        """Check the whole path a tournament takes: the binary, the device, the assembler and a
        war with a known winner. Answers 503 with the failed checks."""
        checks = {}

        def check(name: str, probe) -> None:
            started = time.monotonic()
            try:
                problem = probe()
            except Exception as error:  # a failed probe is the finding, whatever it raised
                problem = str(error) or type(error).__name__
            checks[name] = {"ok": not problem, "seconds": round(time.monotonic() - started, 2)}
            if problem:
                checks[name]["error"] = problem

        def device() -> str | None:
            if self.device == ["--cpu"]:
                return None
            listed = breed.discrete_gpus()
            missing = [i for i in self.device[1].split(",") if int(i) not in listed]
            return f"discrete GPU {', '.join(missing)} is gone, mars gpus lists {listed}" if missing else None

        def play() -> str | None:
            # A bomber against a program that dies on its first step: it must win every war.
            programs = {"bomber": "ADD.AB #4, 1\nMOV.I 2, 2\nJMP -2\nDAT #0, #0\n", "dat": "DAT #0, #0\n"}
            with tempfile.TemporaryDirectory(prefix="mars-health-") as folder:
                folder = Path(folder)
                compiled, files = compile_programs(folder, programs, [])
                if not all(result["ok"] for result in compiled.values()):
                    return f"the probe programs do not assemble: {compiled}"
                out = folder / "runs.jsonl"
                files = [str(path) for path in files.values()]
                # Not queued: two wars fit next to a running tournament.
                played = run_mars("tournament", "--games", "2", "--out", str(out), *self.device, *files)
                if played.returncode != 0:
                    return played.stderr.strip()[-500:]
                run = json.loads(out.read_text())
                return None if run["first_wins"] == 2 else f"wrong result: {run}"

        check("binary", lambda: None if breed.MARS.exists() else f"{breed.MARS} is missing")
        check("device", device)
        check("tournament", play)
        healthy = all(c["ok"] for c in checks.values())
        with self.lock:
            counts = {
                status: sum(job.status == status for job in self.jobs.values()) for status in ("queued", "running")
            }
        report = {"status": "ok" if healthy else "fail", "busy": bool(counts["running"]), **counts, "checks": checks}
        if not healthy:
            raise Refused(503, "unhealthy", **report)
        return report

    def compile(self, body: dict) -> dict:
        with tempfile.TemporaryDirectory(prefix="mars-serve-") as folder:
            return {"programs": compile_programs(Path(folder), sources(body, "programs"), rule_args(body))[0]}

    # --- tournaments ---

    def submit(self, body: dict, wait: float) -> tuple[int, dict]:
        """Queue a full tournament of the programs, or with `against` a gauntlet: every program
        plays every opponent. Programs that don't assemble are left out and reported."""
        candidates, opponents = sources(body, "programs"), sources(body, "against")
        if set(candidates) & set(opponents):
            raise Refused(400, "a program ID is in both programs and against")
        games = body.get("games", 100)
        seed = body.get("seed", secrets.randbits(48))
        label = body.get("label", "")
        if type(games) is not int or type(seed) is not int or games < 1 or seed < 0:
            raise Refused(400, "games must be a positive integer and seed a non-negative one")
        if not isinstance(label, str) or len(label) > MAX_LABEL:
            raise Refused(400, f"label must be a string of at most {MAX_LABEL} characters")
        rules = rule_args(body)
        folder = Path(tempfile.mkdtemp(prefix="mars-serve-"))
        try:
            compiled, files = compile_programs(folder, candidates | opponents, rules)
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
            against = ["--against", *(str(files[name]) for name in second)] if second else []
            out = str(folder / "runs.jsonl")
            args = ["tournament", "--games", str(games), "--seed", str(seed), "--out", out, *rules, *self.device]
            args += [str(files[name]) for name in first] + against
            job = Job(label, seed, games, pairs * games, candidates, opponents, errors, files, args)
            with self.lock:
                self.prune()
                if sum(j.status == "queued" for j in self.jobs.values()) >= self.max_queued:
                    raise Refused(429, f"the queue is full: {self.max_queued} tournaments are waiting")
                job.folder = folder
                self.jobs[job.id] = job
        except BaseException:
            shutil.rmtree(folder, ignore_errors=True)
            raise
        self.queue.put(job)
        return self.poll(job.id, wait)

    def work(self) -> None:
        """The worker thread: plays the queued tournaments one by one."""
        while True:
            job = self.queue.get()
            try:
                self.play(job)
            except Exception as error:  # the worker must outlive any one tournament
                with self.lock:
                    job.status, job.error = "failed", str(error) or type(error).__name__
            finally:
                shutil.rmtree(job.folder, ignore_errors=True)
                with self.lock:
                    job.finished = job.finished or now()
                    job.clock = time.monotonic()
                job.over.set()

    def play(self, job: Job) -> None:
        with self.lock:
            if job.status == "canceled":
                return
            job.status, job.started, job.clock = "running", now(), time.monotonic()
            job.process = subprocess.Popen(
                [str(breed.MARS), *job.args], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True
            )
        timer = threading.Timer(self.job_timeout, job.process.kill)
        timer.start()
        last = deque(maxlen=5)
        for line in job.process.stderr:  # mars reports "DONE/TOTAL wars" every few seconds
            if progress := re.match(r"(\d+)/\d+ wars", line):
                job.wars_done = int(progress[1])
            else:
                last.append(line.strip())
        code = job.process.wait()
        job.process.stderr.close()
        expired = not timer.is_alive()
        timer.cancel()
        runs = [json.loads(line) for line in (job.folder / "runs.jsonl").read_text().splitlines()] if code == 0 else []
        for run in runs:
            run["first"], run["second"] = job.ids[run["first"]], job.ids[run["second"]]
        with self.lock:
            if job.status == "canceled":
                return
            job.seconds = round(time.monotonic() - job.clock, 2)
            if code == 0:
                job.status, job.runs, job.wars_done = "done", runs, job.wars
            elif expired:
                job.status, job.error = "failed", f"took longer than {self.job_timeout:.0f} seconds"
            else:
                job.status, job.error = "failed", " ".join(last).removeprefix("error: ") or f"mars exited with {code}"

    def prune(self) -> None:
        """Forget the tournaments that ended more than `keep` seconds ago. Call with the lock held."""
        horizon = time.monotonic() - self.keep
        for job in [j for j in self.jobs.values() if j.over.is_set() and j.clock < horizon]:
            del self.jobs[job.id]

    def find(self, job_id: str) -> Job:
        with self.lock:
            job = self.jobs.get(job_id)
        if not job:
            raise Refused(404, "no such tournament: it never existed, was deleted, or ended too long ago")
        return job

    def poll(self, job_id: str, wait: float) -> tuple[int, dict]:
        """The tournament's state, after waiting for it to be over for at most `wait` seconds."""
        job = self.find(job_id)
        job.over.wait(wait)
        with self.lock:
            view = job.view(results=True)
            if job.status == "queued":
                view["position"] = [j.id for j in self.jobs.values() if j.status == "queued"].index(job.id) + 1
        return (200 if job.status in OVER else 202), view

    def list(self) -> dict:
        with self.lock:
            self.prune()
            return {"tournaments": [job.view(results=False) for job in self.jobs.values()]}

    def cancel(self, job_id: str) -> dict:
        """Cancel a queued or running tournament. One that is over is forgotten instead."""
        job = self.find(job_id)
        with self.lock:
            if job.status in OVER:
                self.jobs.pop(job.id, None)
            else:
                if job.process:
                    job.process.terminate()
                else:  # still queued: the worker skips it when its turn comes
                    job.over.set()
                job.status, job.finished, job.clock = "canceled", now(), time.monotonic()
            return job.view(results=False)


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

    def route(self, method: str, path: str, body: dict, wait: float) -> tuple[int, dict]:
        server = self.server
        match (method, *path.strip("/").split("/")):
            case ("GET", ""):
                return 200, server.info()
            case ("GET", "livez"):
                return 200, {"status": "ok"}
            case ("GET", "health"):
                return 200, server.health()
            case ("POST", "compile"):
                return 200, server.compile(body)
            case ("POST", "tournaments"):
                return server.submit(body, wait)
            case ("GET", "tournaments"):
                return 200, server.list()
            case ("GET", "tournaments", job_id):
                return server.poll(job_id, wait)
            case ("DELETE", "tournaments", job_id):
                return 200, server.cancel(job_id)
        raise Refused(404, "no such resource, see GET /")

    def serve(self, method: str) -> None:
        try:
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_BODY:
                self.close_connection = True  # the body stays unread
                raise Refused(413, f"the request body is larger than {MAX_BODY} bytes")
            raw = self.rfile.read(length)
            url = urlsplit(self.path)
            if url.path.rstrip("/") != "/livez" and not self.authorized():
                raise Refused(401, "missing or wrong key")
            try:
                body = json.loads(raw) if raw else {}
                wait = float(parse_qs(url.query).get("wait", ["0"])[0])
            except ValueError:
                raise Refused(400, "the body is not JSON, or wait is not a number")
            if not isinstance(body, dict) or not 0 <= wait <= MAX_WAIT:
                raise Refused(400, f"the body must be a JSON object and wait 0 to {MAX_WAIT} seconds")
            self.reply(*self.route(method, url.path, body, wait))
        except Refused as refused:
            self.reply(refused.status, refused.body)

    def do_GET(self) -> None:
        self.serve("GET")

    def do_POST(self) -> None:
        self.serve("POST")

    def do_DELETE(self) -> None:
        self.serve("DELETE")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default="127.0.0.1", help="address to listen on (127.0.0.1)")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--gpu", help="discrete GPUs to use, like 1 or 0,1, or cpu (default: every discrete GPU)")
    parser.add_argument("--max-wars", type=int, default=20_000_000, help="wars per tournament (20000000)")
    parser.add_argument("--max-queued", type=int, default=10, help="tournaments that may wait in the queue (10)")
    parser.add_argument("--timeout", type=float, default=60, help="minutes a tournament may play (60)")
    parser.add_argument("--keep", type=float, default=60, help="minutes a finished tournament stays available (60)")
    args = parser.parse_args()
    key = os.environ.get("MARS_API_KEY", "")
    if len(key) < 16:
        raise SystemExit("set MARS_API_KEY to the pre-shared key, at least 16 characters")
    if not breed.MARS.exists():
        raise SystemExit(f"{breed.MARS} is missing, build it with: cargo build --release")
    device = breed.device_args(args.gpu)
    limits = (args.max_wars, args.max_queued, args.timeout * 60, args.keep * 60)
    server = Server((args.host, args.port), key, device, *limits)
    print(f"serving on http://{args.host}:{server.server_port} with mars {' '.join(server.device)}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    main()
