"""Tests of breed.py and benchmark/fetch.py: python3 -m unittest discover -s Modern/Breeding

The breeding loop runs against a serve.py started on a free local port, with a stand-in for the
LLM. They play a few thousand short wars. MARS_TEST_GPUS pins them to the given adapters, like the GPU
tests of the engine. Without it they use what breed.py would: every discrete GPU, or the CPU.
"""

import contextlib
import io
import json
import os
import random
import stat
import tarfile
import tempfile
import threading
import unittest
from pathlib import Path
from unittest import mock

import breed
import serve
from benchmark import fetch

GPU = os.environ.get("MARS_TEST_GPUS")


class Selection(unittest.TestCase):
    WINNERS = [f"w{n}" for n in range(10)]

    def test_rows_select_a_few_winners_in_random_order(self):
        matrix = breed.selection(random.Random("7-3"), 200, self.WINNERS, 2, 0)
        self.assertEqual(len(matrix), 200)
        for row in matrix:
            chosen = [w for w, bit in zip(self.WINNERS, row["select"]) if bit]
            self.assertEqual(set(row["select"]) | {0, 1}, {0, 1})
            self.assertTrue(2 <= len(chosen) <= 5)  # up to half of the winners
            self.assertEqual(sorted(row["order"]), chosen)
        self.assertEqual({sum(row["select"]) for row in matrix}, {2, 3, 4, 5})
        self.assertTrue(any(row["order"] != sorted(row["order"]) for row in matrix))
        self.assertEqual(
            {sum(row["select"]) for row in breed.selection(random.Random(1), 50, self.WINNERS, 3, 4)}, {3, 4}
        )

    def test_the_seed_repeats_the_matrix(self):
        draw = lambda seed: breed.selection(random.Random(seed), 20, self.WINNERS, 2, 0)
        self.assertEqual(draw("7-3"), draw("7-3"))
        self.assertNotEqual(draw("7-3"), draw("7-4"))

    def test_few_or_no_winners(self):
        self.assertEqual(breed.selection(random.Random(1), 2, [], 2, 0), [{"select": [], "order": []}] * 2)
        self.assertEqual(breed.selection(random.Random(1), 1, ["only"], 2, 0), [{"select": [1], "order": ["only"]}])


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


@unittest.skipUnless(breed.MARS.exists(), "needs the release build of mars")
class Loop(unittest.TestCase):
    """The breeding loop against a local serve.py. The stand-in LLM writes bombers with random steps."""

    KEY = "test-key-0123456789"

    def setUp(self):
        self.tmp = Path(self.enterContext(tempfile.TemporaryDirectory()))
        server = serve.Server(("127.0.0.1", 0), self.KEY, breed.device_args(GPU), max_wars=100000)
        server.RequestHandlerClass.log_message = lambda *args: None
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        remote = {"MARS_URL": f"http://127.0.0.1:{server.server_port}/", "MARS_API_KEY": self.KEY}
        self.enterContext(mock.patch.dict(os.environ, remote))
        self.enterContext(mock.patch.object(breed, "RUNS", self.tmp))
        self.rng = random.Random(5)

    def step(self, **options) -> tuple[int, str]:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = breed.Run("test", options).step()
        return code, output.getvalue()

    def write(self, run: Path, programs: list[str]) -> None:
        for program in programs:
            step = self.rng.randrange(1, 8000)
            source = f";name {program}\nADD.AB #{step}, 1\nMOV.I 2, 2\nJMP -2\nDAT #0, #0\n"
            (run / "programs" / f"{program}.red").write_text(source)

    def test_three_generations(self):
        run = self.tmp / "test"
        code, output = self.step(population=10, games=4, generations=3, seed=11)
        state = lambda: json.loads((run / "state.json").read_text())
        first = [f"g001-r{n:03d}" for n in range(1, 11)]
        self.assertEqual((code, state()["generation"], state()["pending"]), (0, 1, first))
        self.assertIn("Write these 10 programs", output)
        prompt = (run / "prompts" / "g001-r001.md").read_text()
        self.assertIn("There are no earlier warriors yet", prompt)
        self.assertIn("/programs/g001-r001.red`", prompt)
        self.assertTrue(any(family in prompt for family in breed.FAMILIES))

        # Nothing is played until every program is written.
        self.write(run, first[:8])
        (run / "programs" / "g001-r009.red").write_text("MOV 0, nowhere\n")
        code, output = self.step()
        self.assertEqual((code, "1 programs of generation 1 are not written yet" in output), (0, True))
        self.assertIn("g001-r010.md", output)
        self.write(run, first[9:])

        # The server validates them first. One that does not assemble is replaced, not repaired:
        # it is set aside, its row is drawn again, and only its new prompt is waiting.
        old_prompt = (run / "prompts" / "g001-r009.md").read_text()
        code, output = self.step()
        self.assertEqual(code, 0)
        self.assertIn("g001-r009: Undefined symbol at line 1", output)
        self.assertIn("Write these 1 programs", output)
        self.assertEqual((state()["generation"], state()["history"]), (1, []))
        self.assertFalse((run / "programs" / "g001-r009.red").exists())
        self.assertEqual((run / "rejected" / "g001-r009.1.red").read_text(), "MOV 0, nowhere\n")
        self.assertNotEqual(
            (run / "prompts" / "g001-r009.md").read_text(), old_prompt
        )  # another strategy to start from
        rejected = json.loads((run / "generations" / "001.json").read_text())["rejected"]
        self.assertEqual([(r["program"], r["errors"][0][:16]) for r in rejected], [("g001-r009", "Undefined symbol")])
        self.assertIn("1 programs of generation 1 are not written yet", self.step()[1])
        self.write(run, ["g001-r009"])

        code, output = self.step()
        self.assertEqual(code, 0, output)
        self.assertEqual((state()["generation"], len(state()["winners"]), len(state()["pending"])), (2, 2, 10))
        record = json.loads((run / "generations" / "001.json").read_text())
        ranked = [row["program"] for row in record["standings"]]
        self.assertEqual(len(ranked), 10 + 4)  # the seeds play in the first generation
        self.assertEqual(state()["winners"], ranked[:2])
        self.assertEqual(
            [row["elo"] for row in record["standings"]],
            sorted((row["elo"] for row in record["standings"]), reverse=True),
        )
        report = (run / "reports" / "001.md").read_text()
        self.assertIn("| 1 | " + ranked[0], report)
        self.assertEqual(report.count("| kept |"), 2)
        self.assertIn("Programs replaced because they did not assemble: 1", report)
        self.assertEqual((run / "champion.red").read_text(), (run / "programs" / f"{ranked[0]}.red").read_text())

        # The second generation combines the winners: the matrix, and a prompt with their sources.
        plan = json.loads((run / "generations" / "002.json").read_text())
        self.assertEqual(plan["winners"], state()["winners"])
        self.assertEqual([row["select"] for row in plan["matrix"]], [[1, 1]] * 10)
        row = plan["matrix"][0]
        prompt = (run / "prompts" / f"{row['program']}.md").read_text()
        self.assertIn("combines the ideas of all of them", prompt)
        for number, parent in enumerate(row["order"], 1):
            self.assertIn(f"## Warrior {number}: {parent} (rank ", prompt)
            self.assertIn((run / "programs" / f"{parent}.red").read_text().rstrip(), prompt)
        self.assertEqual(state()["programs"][row["program"]], {"generation": 2, "parents": row["order"]})

        # A replaced program of a later generation gets a row drawn again from the same winners.
        self.write(run, state()["pending"])
        (run / "programs" / "g002-r003.red").write_text("JMP nowhere\n")
        self.assertIn("are replaced, not repaired", self.step()[1])
        redrawn = json.loads((run / "generations" / "002.json").read_text())
        self.assertEqual((redrawn["matrix"][2]["program"], redrawn["matrix"][2]["select"]), ("g002-r003", [1, 1]))
        self.assertIn("## Warrior 2: ", (run / "prompts" / "g002-r003.md").read_text())
        self.assertTrue((run / "rejected" / "g002-r003.2.red").exists())

        # The winners play again: 10 new programs and 2 winners, without the other seeds.
        self.write(run, ["g002-r003"])
        self.assertEqual(self.step()[0], 0)
        self.assertEqual(len(json.loads((run / "generations" / "002.json").read_text())["standings"]), 12)
        self.write(run, state()["pending"])
        code, output = self.step()
        self.assertEqual((code, state()["generation"], state()["pending"]), (0, 3, []))
        self.assertIn("STOP: 3 generations played", output)
        self.assertIn("STOP: 3 generations played", self.step()[1])
        self.assertEqual([h["generation"] for h in state()["history"]], [1, 2, 3])
        self.assertEqual(breed.best_of("test", 1), [run / "programs" / f"{state()['winners'][0]}.red"])

    def test_the_server_and_the_key_are_required(self):
        with mock.patch.dict(os.environ, {"MARS_API_KEY": "wrong-key-0123456789"}):
            with self.assertRaisesRegex(SystemExit, "401 missing or wrong key"):
                breed.api("GET", "/")
        with mock.patch.dict(os.environ, {"MARS_API_USER": "agent"}):
            self.assertEqual(breed.api("GET", "/")["service"], "corewar mars")
        with mock.patch.dict(os.environ, {"MARS_URL": ""}), self.assertRaisesRegex(SystemExit, "set MARS_URL"):
            breed.api("GET", "/")


if __name__ == "__main__":
    unittest.main()
