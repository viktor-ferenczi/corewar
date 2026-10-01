"""Tests of serve.py: python3 -m unittest discover -s Modern/Breeding

A server on a free local port plays a few short tournaments. MARS_TEST_GPUS pins it to the given
adapters; without it the server uses what breed.py would.
"""

import base64
import json
import os
import threading
import time
import unittest
import urllib.error
import urllib.request
from unittest import mock

import breed
import serve

KEY = "test-key-0123456789"
STONE = "ADD.AB #3044, 1\nMOV.I 2, 2\nJMP -2\nDAT #0, #0\n"
CLEAR = "ORG 1\nDAT #0, #12\nSPL #0\nMOV 2, >-2\nJMP -1, >-3\nDAT <2667, <5334\n"
SITTER = "JMP 0\n"
BROKEN = "MOV 0, nowhere\n"


@unittest.skipUnless(breed.MARS.exists(), "needs the release build of mars")
class Api(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        device = breed.device_args(os.environ.get("MARS_TEST_GPUS"))
        cls.server = serve.Server(("127.0.0.1", 0), KEY, device, max_wars=100000, max_queued=2)
        cls.server.RequestHandlerClass.log_message = lambda *args: None
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.addClassCleanup(cls.server.server_close)
        cls.addClassCleanup(cls.server.shutdown)

    def call(self, path: str, body=None, authorization: str | None = f"Bearer {KEY}", method=None) -> tuple[int, dict]:
        data = body if isinstance(body, bytes) or body is None else json.dumps(body).encode()
        url = f"http://127.0.0.1:{self.server.server_port}{path}"
        request = urllib.request.Request(url, data=data, method=method)
        if authorization:
            request.add_header("Authorization", authorization)
        try:
            with urllib.request.urlopen(request, timeout=90) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def play(self, body: dict) -> dict:
        """Submit a tournament and long poll until it is over."""
        status, job = self.call("/tournaments?wait=60", body)
        self.assertEqual((status, job["status"]), (200, "done"), job)
        return job

    def long_tournament(self, label: str) -> str:
        """Submit a tournament of drawn wars that would take many seconds. Returns its ID."""
        body = {"programs": {"a": SITTER, "b": SITTER}, "games": 60000, "label": label}
        status, job = self.call("/tournaments", body)
        self.assertEqual((status, job["label"], job["progress"]), (202, label, 0.0))
        self.addCleanup(self.call, f"/tournaments/{job['id']}", method="DELETE")
        return job["id"]

    def wait_for(self, job_id: str, wanted: str) -> dict:
        for _ in range(100):
            job = self.call(f"/tournaments/{job_id}")[1]
            if job["status"] == wanted:
                return job
            time.sleep(0.05)
        self.fail(f"{job_id} is {job['status']}, not {wanted}")

    def test_the_key_is_required(self):
        basic = lambda password: "Basic " + base64.b64encode(f"agent:{password}".encode()).decode()
        for authorization in (None, "Bearer wrong", basic("wrong"), "Basic !!!", f"Digest {KEY}"):
            self.assertEqual(self.call("/", authorization=authorization)[0], 401, authorization)
        self.assertEqual(self.call("/tournaments", {"programs": {}}, authorization=None)[0], 401)
        self.assertEqual(self.call("/tournaments", authorization=None)[0], 401)
        for authorization in (f"Bearer {KEY}", basic(KEY)):
            status, info = self.call("/", authorization=authorization)
            self.assertEqual((status, info["max_wars"], info["max_queued"]), (200, 100000, 2))
            self.assertEqual(info["device"], " ".join(self.server.device))

    def test_livez_needs_no_key_and_health_does(self):
        self.assertEqual(self.call("/livez", authorization=None), (200, {"status": "ok"}))
        self.assertEqual(self.call("/health", authorization=None)[0], 401)
        status, body = self.call("/health")
        self.assertEqual((status, body["status"]), (200, "ok"))
        self.assertEqual(list(body["checks"]), ["binary", "device", "tournament"])
        self.assertTrue(all(check["ok"] for check in body["checks"].values()))

    def test_health_reports_what_failed(self):
        with mock.patch.object(serve, "run_mars", side_effect=serve.Refused(504, "mars took too long")):
            status, body = self.call("/health")
        self.assertEqual((status, body["status"], body["error"]), (503, "fail", "unhealthy"))
        self.assertEqual(body["checks"]["tournament"]["error"], "mars took too long")
        self.assertTrue(body["checks"]["binary"]["ok"])

    def test_compile(self):
        status, body = self.call("/compile", {"programs": {"stone": STONE, "broken": BROKEN}})
        self.assertEqual(status, 200)
        stone, broken = body["programs"]["stone"], body["programs"]["broken"]
        self.assertEqual((stone["ok"], stone["start"], len(stone["instructions"])), (True, 0, 4))
        first = {"op": "ADD", "modifier": "AB", "a_mode": "#", "a": 3044, "b_mode": "$", "b": 1}
        self.assertEqual(stone["instructions"][0], first)
        self.assertEqual(broken, {"ok": False, "errors": ["Undefined symbol at line 1 in program broken !"]})

    def test_full_tournament_leaves_out_what_does_not_assemble(self):
        programs = {"stone": STONE, "clear": CLEAR, "sitter": SITTER, "broken": BROKEN}
        request = {"programs": programs, "games": 20, "seed": 5, "steps": 20000, "label": "generation 7"}
        job = self.play(request)
        self.assertEqual((job["seed"], job["games"], job["wars"], job["label"]), (5, 20, 60, "generation 7"))
        self.assertEqual((job["progress"], job["wars_done"]), (100.0, 60))
        self.assertEqual(list(job["errors"]), ["broken"])
        pairs = [(r["first"], r["second"]) for r in job["runs"]]
        self.assertEqual(pairs, [("stone", "clear"), ("stone", "sitter"), ("clear", "sitter")])
        self.assertTrue(all(r["max_steps"] == 20000 and r["standard"] == "pmars" for r in job["runs"]))
        rows = job["standings"]
        self.assertEqual({row["id"] for row in rows}, {"stone", "clear", "sitter"})
        self.assertTrue(all(row["games"] == 40 == row["wins"] + row["draws"] + row["losses"] for row in rows))
        self.assertEqual(rows, sorted(rows, key=lambda row: -row["score"]))
        self.assertEqual(rows[-1]["id"], "sitter")
        # What was sent comes back with the results.
        self.assertEqual((job["programs"], job["against"]), (programs, {}))
        # The same seed repeats the tournament, and a seed is drawn when none is given.
        self.assertEqual(self.play(request)["runs"], job["runs"])
        self.assertNotEqual(self.play({"programs": programs, "games": 20})["seed"], 5)

    def test_gauntlet_with_free_text_ids(self):
        stone, clear, sitter = "--out", "../../etc/x y", '3f2c9a\u00fc "q"'
        request = {
            "programs": {stone: STONE, clear: CLEAR, "bad one": BROKEN},
            "against": {sitter: SITTER},
            "games": 10,
        }
        job = self.play(request)
        self.assertEqual([(r["first"], r["second"]) for r in job["runs"]], [(stone, sitter), (clear, sitter)])
        self.assertEqual({row["id"] for row in job["standings"]}, {stone, clear})
        self.assertEqual(job["errors"], {"bad one": ["Undefined symbol at line 1 in program bad one !"]})
        self.assertEqual((job["programs"], job["against"]), (request["programs"], request["against"]))
        self.assertEqual(job["wars"], 20)

    def test_queue_progress_cancel_and_catching_up(self):
        quick = {"programs": {"stone": STONE, "clear": CLEAR}, "games": 4}
        first = self.long_tournament("long")
        self.assertEqual(self.wait_for(first, "running")["started"] is not None, True)
        second = self.call("/tournaments", quick | {"label": "second"})
        third = self.call("/tournaments", quick | {"label": "third"})
        self.assertEqual(
            [(s, job["status"], job["position"]) for s, job in (second, third)],
            [(202, "queued", 1)] + [(202, "queued", 2)],
        )
        full = self.call("/tournaments", quick)
        self.assertEqual((full[0], "queue is full" in full[1]["error"]), (429, True))
        self.assertTrue(self.call("/health")[1]["busy"])

        # A restarted agent finds its tournaments by their labels. The list has no results.
        listed = [
            job for job in self.call("/tournaments")[1]["tournaments"] if job["label"] in ("long", "second", "third")
        ]
        self.assertEqual(
            [(job["label"], job["status"]) for job in listed],
            [("long", "running"), ("second", "queued"), ("third", "queued")],
        )
        self.assertTrue(
            all(0 <= job["progress"] < 100 and "runs" not in job and "programs" not in job for job in listed)
        )
        # A long poll returns after its wait when the tournament is still going.
        started = time.monotonic()
        self.assertEqual(self.call(f"/tournaments/{third[1]['id']}?wait=0.3")[0], 202)
        self.assertGreaterEqual(time.monotonic() - started, 0.3)

        # Cancel the queued second and the running first: the third still plays.
        for job_id in (second[1]["id"], first):
            status, job = self.call(f"/tournaments/{job_id}", method="DELETE")
            self.assertEqual((status, job["status"], job["finished"] is not None), (200, "canceled", True))
        status, job = self.call(f"/tournaments/{third[1]['id']}?wait=60")
        self.assertEqual((status, job["status"], job["progress"], len(job["runs"])), (200, "done", 100.0, 1))
        self.assertEqual(self.call(f"/tournaments/{first}?wait=5")[1]["status"], "canceled")
        self.assertEqual(self.call(f"/tournaments/{second[1]['id']}")[1]["status"], "canceled")

        # Deleting a tournament that is over forgets it.
        self.assertEqual(self.call(f"/tournaments/{third[1]['id']}", method="DELETE")[0], 200)
        self.assertEqual(self.call(f"/tournaments/{third[1]['id']}")[0], 404)
        self.assertNotIn(third[1]["id"], [job["id"] for job in self.call("/tournaments")[1]["tournaments"]])

    def test_finished_tournaments_are_forgotten_after_a_while(self):
        job = self.play({"programs": {"stone": STONE, "clear": CLEAR}, "games": 2})
        with mock.patch.object(self.server, "keep", 0):
            listed = self.call("/tournaments")[1]["tournaments"]
        self.assertNotIn(job["id"], [j["id"] for j in listed])

    def test_refused_requests(self):
        two = {"stone": STONE, "clear": CLEAR}
        for expected, path, body in (
            (404, "/nowhere", {}),
            (404, "/tournaments/not-an-id", None),
            (400, "/tournaments", b"not json"),
            (400, "/tournaments", {"programs": ["stone"]}),
            (400, "/tournaments", {"programs": {"": STONE, "clear": CLEAR}}),
            (400, "/tournaments", {"programs": {"x" * 201: STONE, "clear": CLEAR}}),
            (400, "/tournaments", {"programs": two, "against": {"stone": STONE}}),
            (400, "/tournaments", {"programs": two, "standard": "--quirks"}),
            (400, "/tournaments", {"programs": two, "core": "8000"}),
            (400, "/tournaments", {"programs": two, "core": 50}),  # refused by mars itself
            (400, "/tournaments", {"programs": two, "games": 0}),
            (400, "/tournaments", {"programs": two, "label": 5}),
            (400, "/tournaments?wait=soon", {"programs": two}),
            (400, "/tournaments?wait=301", {"programs": two}),
            (400, "/compile", {"programs": {"big": "x" * (serve.MAX_SOURCE + 1)}}),
            (413, "/tournaments", {"programs": two, "games": 60000, "against": {"a": SITTER}}),
            (422, "/tournaments", {"programs": {"stone": STONE, "broken": BROKEN}}),
            (422, "/tournaments", {"programs": {"stone": STONE}}),
        ):
            status, reply = self.call(path, body)
            self.assertEqual(
                status, expected, (path, body if isinstance(body, (bytes, type(None))) else list(body), reply)
            )
            self.assertIn("error", reply)
        self.assertIn("--core must be", self.call("/tournaments", {"programs": two, "core": 50})[1]["error"])
        broken = self.call("/tournaments", {"programs": {"stone": STONE, "broken": BROKEN}})[1]
        self.assertEqual(list(broken["errors"]), ["broken"])


if __name__ == "__main__":
    unittest.main()
