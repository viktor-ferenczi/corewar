"""Tests of serve.py: python3 -m unittest discover -s Modern/Breeding

A server on a free local port plays a few short tournaments. MARS_TEST_GPUS pins it to the given
adapters; without it the server uses what breed.py would.
"""

import base64
import json
import os
import threading
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
        cls.server = serve.Server(("127.0.0.1", 0), KEY, device, max_wars=1000)
        cls.server.RequestHandlerClass.log_message = lambda *args: None
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.addClassCleanup(cls.server.server_close)
        cls.addClassCleanup(cls.server.shutdown)

    def call(self, path: str, body=None, authorization: str | None = f"Bearer {KEY}") -> tuple[int, dict]:
        data = body if isinstance(body, bytes) or body is None else json.dumps(body).encode()
        request = urllib.request.Request(f"http://127.0.0.1:{self.server.server_port}{path}", data=data)
        if authorization:
            request.add_header("Authorization", authorization)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def test_the_key_is_required(self):
        basic = lambda password: "Basic " + base64.b64encode(f"agent:{password}".encode()).decode()
        for authorization in (None, "Bearer wrong", basic("wrong"), "Basic !!!", f"Digest {KEY}"):
            self.assertEqual(self.call("/", authorization=authorization)[0], 401, authorization)
        self.assertEqual(self.call("/tournaments", {"programs": {}}, authorization=None)[0], 401)
        for authorization in (f"Bearer {KEY}", basic(KEY)):
            status, info = self.call("/", authorization=authorization)
            self.assertEqual((status, info["max_wars"], info["device"]), (200, 1000, " ".join(self.server.device)))

    def test_livez_needs_no_key_and_health_does(self):
        self.assertEqual(self.call("/livez", authorization=None), (200, {"status": "ok"}))
        self.assertEqual(self.call("/health", authorization=None)[0], 401)
        status, body = self.call("/health")
        self.assertEqual((status, body["status"], body["busy"]), (200, "ok", False))
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
        self.assertEqual(
            stone["instructions"][0], {"op": "ADD", "modifier": "AB", "a_mode": "#", "a": 3044, "b_mode": "$", "b": 1}
        )
        self.assertEqual(broken, {"ok": False, "errors": ["Undefined symbol at line 1 in program broken !"]})

    def test_full_tournament_leaves_out_what_does_not_assemble(self):
        programs = {"stone": STONE, "clear": CLEAR, "sitter": SITTER, "broken": BROKEN}
        status, body = self.call("/tournaments", {"programs": programs, "games": 20, "seed": 5, "steps": 20000})
        self.assertEqual(status, 200)
        self.assertEqual((body["seed"], body["games"], body["wars"]), (5, 20, 60))
        self.assertEqual(list(body["errors"]), ["broken"])
        self.assertEqual(
            [(r["first"], r["second"]) for r in body["runs"]],
            [("stone", "clear"), ("stone", "sitter"), ("clear", "sitter")],
        )
        self.assertTrue(all(r["max_steps"] == 20000 and r["standard"] == "pmars" for r in body["runs"]))
        rows = body["standings"]
        self.assertEqual({row["name"] for row in rows}, {"stone", "clear", "sitter"})
        self.assertTrue(all(row["games"] == 40 == row["wins"] + row["draws"] + row["losses"] for row in rows))
        self.assertEqual(rows, sorted(rows, key=lambda row: -row["score"]))
        self.assertEqual(rows[-1]["name"], "sitter")
        # The same seed repeats the tournament, and a seed is drawn when none is given.
        self.assertEqual(
            self.call("/tournaments", {"programs": programs, "games": 20, "seed": 5, "steps": 20000})[1]["runs"],
            body["runs"],
        )
        self.assertNotEqual(self.call("/tournaments", {"programs": programs, "games": 20})[1]["seed"], 5)

    def test_gauntlet(self):
        request = {"programs": {"stone": STONE, "clear": CLEAR}, "against": {"sitter": SITTER}, "games": 10}
        status, body = self.call("/tournaments", request)
        self.assertEqual(status, 200)
        self.assertEqual([(r["first"], r["second"]) for r in body["runs"]], [("stone", "sitter"), ("clear", "sitter")])
        self.assertEqual({row["name"] for row in body["standings"]}, {"stone", "clear"})
        self.assertEqual(body["wars"], 20)

    def test_refused_requests(self):
        two = {"stone": STONE, "clear": CLEAR}
        for expected, path, body in (
            (404, "/nowhere", {}),
            (400, "/tournaments", b"not json"),
            (400, "/tournaments", {"programs": ["stone"]}),
            (400, "/tournaments", {"programs": {"--out": STONE, "clear": CLEAR}}),
            (400, "/tournaments", {"programs": {"../x": STONE, "clear": CLEAR}}),
            (400, "/tournaments", {"programs": two, "against": {"stone": STONE}}),
            (400, "/tournaments", {"programs": two, "standard": "--quirks"}),
            (400, "/tournaments", {"programs": two, "core": "8000"}),
            (400, "/tournaments", {"programs": two, "core": 50}),  # refused by mars itself
            (400, "/tournaments", {"programs": two, "games": 0}),
            (400, "/compile", {"programs": {"big": "x" * (serve.MAX_SOURCE + 1)}}),
            (413, "/tournaments", {"programs": two, "games": 1001}),
            (422, "/tournaments", {"programs": {"stone": STONE, "broken": BROKEN}}),
            (422, "/tournaments", {"programs": {"stone": STONE}}),
        ):
            status, reply = self.call(path, body)
            self.assertEqual(status, expected, (body if isinstance(body, bytes) else list(body), reply))
            self.assertIn("error", reply)
        self.assertIn("--core must be", self.call("/tournaments", {"programs": two, "core": 50})[1]["error"])
        self.assertEqual(
            list(self.call("/tournaments", {"programs": {"stone": STONE, "broken": BROKEN}})[1]["errors"]), ["broken"]
        )


if __name__ == "__main__":
    unittest.main()
