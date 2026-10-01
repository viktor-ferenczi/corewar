"""Tests of breed.py and benchmark/fetch.py: python3 -m unittest discover -s Modern/Breeding

They play a few thousand short wars. MARS_TEST_GPUS pins them to the given adapters, like the GPU
tests of the engine. Without it they use what breed.py would: every discrete GPU, or the CPU.
"""

import io
import json
import os
import random
import stat
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import breed
from benchmark import fetch

GPU = os.environ.get("MARS_TEST_GPUS")


class Operators(unittest.TestCase):
    def test_children_are_valid_programs(self):
        rng = random.Random(1)
        parents = [
            {"id": f"w{n}", "code": code, "start": start}
            for n, (code, start) in enumerate(breed.random_program(rng) for _ in range(5))
        ]
        children = breed.breed(rng, parents, 300)
        self.assertEqual(len(children), 300)
        self.assertEqual({c["op"] for c in children}, {"mutate", "cross", "sweep"})
        for child in children:
            self.assertTrue(1 <= len(child["code"]) <= breed.MAX_LENGTH)
            self.assertTrue(0 <= child["start"] < len(child["code"]))
            self.assertTrue(set(child["parents"]) <= {p["id"] for p in parents})
            for op, modifier, a_mode, a, b_mode, b in child["code"]:
                self.assertIn(op, breed.OPCODES)
                self.assertIn(modifier, breed.MODIFIERS)
                self.assertTrue(a_mode in breed.MODES and b_mode in breed.MODES)
                self.assertTrue(0 <= a < breed.CORE and 0 <= b < breed.CORE)

    def test_crossover_respects_the_length_limit(self):
        rng = random.Random(2)
        long = [breed.random_instruction(rng) for _ in range(breed.MAX_LENGTH)]
        for _ in range(200):
            code, start = breed.crossover(rng, long, long, 99)
            self.assertTrue(1 <= len(code) <= breed.MAX_LENGTH and start < len(code))

    def test_no_parents_gives_random_programs(self):
        self.assertEqual({c["op"] for c in breed.breed(random.Random(3), [], 10)}, {"random"})

    def test_cells(self):
        self.assertEqual(breed.cell_of(1, 0.0), "0-0")
        self.assertEqual(breed.cell_of(6, 1.5), "1-1")
        self.assertEqual(breed.cell_of(100, 5000.0), "4-4")


class Statistics(unittest.TestCase):
    def test_tally(self):
        tally = breed.Tally([[6, 2, 2, 30], [0, 0, 10, 0]])
        self.assertEqual((tally.games, tally.mean, tally.koth, tally.processes), (20, 0.35, 100.0, 1.5))
        self.assertAlmostEqual(tally.error, ((6.5 / 20 - 0.35**2) / 20) ** 0.5)
        better = breed.Tally([[900, 0, 100, 0]])
        worse = breed.Tally([[500, 0, 500, 0]])
        self.assertGreater(breed.z_score(better, worse), 2)
        self.assertEqual(breed.z_score(worse, worse), 0)


class Devices(unittest.TestCase):
    def test_only_discrete_gpus(self):
        with mock.patch.object(breed, "discrete_gpus", return_value=[0, 1]), mock.patch.dict(os.environ, clear=True):
            self.assertEqual(breed.device_args(None), ["--gpu", "0,1"])
            self.assertEqual(breed.device_args("1"), ["--gpu", "1"])
            self.assertEqual(breed.device_args("cpu"), ["--cpu"])
            with self.assertRaises(SystemExit):
                breed.device_args("2")  # the integrated GPU
            os.environ["BREED_GPUS"] = "0"
            self.assertEqual(breed.device_args(None), ["--gpu", "0"])

    def test_cpu_without_a_discrete_gpu(self):
        with mock.patch.object(breed, "discrete_gpus", return_value=[]), mock.patch.dict(os.environ, clear=True):
            self.assertEqual(breed.device_args(None), ["--cpu"])


class Fetch(unittest.TestCase):
    def test_archive_members_and_ranking(self):
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w:gz") as tar:
            for name, text in (
                ("HILL32/a.red", b";name A\r\n;author Me\r\nMOV 0, 1\n"),
                ("HILL32/._a.red", b"x"),
                ("HILL32/notes.txt", b"x"),
            ):
                info = tarfile.TarInfo(name)
                info.size = len(text)
                tar.addfile(info, io.BytesIO(text))
        files = fetch.members(archive.getvalue())
        self.assertEqual(list(files), ["a.red"])
        self.assertEqual((fetch.header(files["a.red"], "name"), fetch.header(files["a.red"], "author")), ("A", "Me"))
        page = b"<pre>\n   1  <a href='x'>Some Name</a>                 A &amp; B           145.489 (94)\n  2  Other   C  99.5 (94)\n</pre>"
        self.assertEqual(
            fetch.published_ranking(page),
            [
                {"rank": 1, "name": "Some Name", "author": "A & B", "score": 145.489},
                {"rank": 2, "name": "Other", "author": "C", "score": 99.5},
            ],
        )


@unittest.skipUnless(breed.MARS.exists(), "needs the release build of mars")
class Engine(unittest.TestCase):
    """Runs against a stand-in benchmark field made of the seed warriors, in a temporary folder."""

    def setUp(self):
        self.tmp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        seeds = sorted(breed.SEEDS.glob("*.red"))
        field = {
            "source": "test field",
            "page": "nowhere",
            "ranking": "nowhere",
            "pinned": "never",
            "published": [
                {"rank": n, "name": p.stem, "author": "test", "score": 100.0 - n} for n, p in enumerate(seeds, 1)
            ],
            "warriors": [{"file": p.name, "name": p.stem, "author": "test"} for p in seeds],
        }
        for target, value in (("WARRIORS", breed.SEEDS), ("RUNS", self.tmp / "runs"), ("load_field", lambda: field)):
            self.enterContext(mock.patch.object(breed, target, value))
        self.enterContext(mock.patch.object(fetch, "USAGE", self.tmp / "USAGE.md"))

    def test_canonical_source_assembles_to_the_same_program(self):
        rng = random.Random(4)
        code = [breed.random_instruction(rng) for _ in range(50)]
        warrior = {"id": "w1", "code": code, "start": 7, "op": "random", "parents": [], "gen": 0}
        path = self.tmp / "w1.red"
        path.write_text(breed.canonical(warrior))
        broken = self.tmp / "broken.red"
        broken.write_text("MOV 0, nowhere\n")
        good, bad = breed.compile_sources([path, broken])
        self.assertEqual((good["code"], good["start"]), (code, 7))
        self.assertIn("Undefined symbol", bad["error"])

    def test_hu93_sources_become_seeds(self):
        historical = breed.HERE.parent.parent / "Historical" / "PRB004.CWR"
        (compiled,) = breed.compile_sources([historical], hu93=True)
        self.assertTrue(all(ins[1] in breed.MODIFIERS for ins in compiled["code"]))

    def test_two_iterations_with_an_inbox(self):
        options = {"population": 20, "screen_games": 4, "refine_games": 10, "hall_games": 20, "hall_size": 2, "seed": 1}
        run = breed.Run("test", options, GPU)
        inbox = run.dir / "inbox"
        (inbox / "good.red").write_text(";intent a second process keeps the first alive\nSPL 0\nMOV 0, 1\n")
        (inbox / "bad.red").write_text(";intent never assembles\nMOV 0, nowhere\n")
        report = run.iterate(minutes=1, generations=2).read_text()
        self.assertLessEqual(len(report.splitlines()), 150)
        for expected in (
            "## Leaderboard against the benchmark field",
            "| bad.red | rejected: Undefined symbol",
            "| good.red | w0",
            "paper by test",
        ):
            self.assertIn(expected, report)
        self.assertEqual(sorted(p.name for p in (run.dir / "llm" / "iter-001").iterdir()), ["bad.red", "good.red"])
        self.assertEqual(list(inbox.iterdir()), [])

        # Opponents are never parents: every parent is a bred warrior's ID.
        lineage = [json.loads(line) for line in (run.dir / "lineage.jsonl").read_text().splitlines()]
        ids = {row["id"] for row in lineage}
        self.assertTrue(all(set(row["parents"]) <= ids for row in lineage))
        self.assertEqual({row["op"] for row in lineage if not row["parents"]}, {"seed", "llm", "random"})

        again = breed.Run("test", {}, GPU)
        self.assertEqual(again.config["population"], 20)
        self.assertTrue(1 <= len(again.state["hof"]) <= 2)  # the first newcomer always joins
        self.assertTrue(all(p.exists() for p in again.hof_files()))
        again.iterate(minutes=1, generations=1)
        self.assertEqual((again.state["iteration"], again.state["generation"]), (2, 3))
        self.assertTrue((run.dir / "reports" / "iter-002.md").exists())
        usage = (self.tmp / "USAGE.md").read_text().splitlines()
        self.assertEqual(len(usage), 2)
        self.assertIn("test, iteration 2 | local breeding benchmark, opponents only", usage[1])

    def test_rank_and_pmars_check(self):
        with mock.patch.object(breed, "BENCHMARK", self.tmp):
            ranking = breed.rank(None, 0, 20, GPU).read_text()
        self.assertIn("| Published # |", ranking)
        self.assertEqual(sum(" by test " in line for line in ranking.splitlines()), 4)

        pmars = self.tmp / "pmars"
        pmars.write_text("#!/bin/sh\necho 'Results: 5 3 2'\n")
        pmars.chmod(pmars.stat().st_mode | stat.S_IXUSR)
        with mock.patch.dict(os.environ, {"PMARS_092_BIN": str(pmars)}), mock.patch("builtins.print") as printed:
            breed.pmars_check(None, 2, 10)
        self.assertEqual(printed.call_count, 3)
        self.assertIn("  0.600", printed.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
