import hashlib
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from release import api, eligible, package, publish, release_tag


class ReleaseTests(unittest.TestCase):
    def setUp(self) -> None:
        self.run = {
            "id": 42,
            "head_sha": "a" * 40,
            "name": "CI",
            "path": ".github/workflows/ci.yml",
            "status": "completed",
            "conclusion": "success",
            "event": "pull_request",
        }
        self.pull = {"number": 9, "state": "open", "draft": False, "head": {"sha": "a" * 40}}
        self.artifacts = {"artifacts": [{"name": "mars-linux-x86_64", "expired": False}]}

    def test_successful_main_and_ready_pr_only(self) -> None:
        self.assertTrue(eligible(self.run, self.pull))
        self.assertTrue(eligible(self.run | {"event": "push", "head_branch": "main"}))
        for patch in [{"draft": True}, {"state": "closed"}, {"head": {"sha": "b" * 40}}]:
            self.assertFalse(eligible(self.run, self.pull | patch))
        for patch in [
            {"conclusion": "failure"},
            {"status": "in_progress"},
            {"event": "workflow_dispatch"},
            {"path": "other.yml"},
            {"name": "Other"},
        ]:
            self.assertFalse(eligible(self.run | patch, self.pull))
        self.assertFalse(eligible(self.run))
        self.assertFalse(eligible(self.run | {"event": "push", "head_branch": "other"}))

    def test_paginated_api_works_with_older_github_clients(self) -> None:
        with patch("release.subprocess.run", return_value=SimpleNamespace(stdout='{"id":1}\n{"id":2}\n')) as command:
            self.assertEqual(api("owner/repo", "releases", pages=True), [{"id": 1}, {"id": 2}])
        self.assertIn("--paginate", command.call_args.args[0])
        self.assertNotIn("--slurp", command.call_args.args[0])

    def test_versions_start_at_zero_and_include_drafts_and_tags(self) -> None:
        self.assertEqual(release_tag([], [], 42), ("v0.1.0", None))
        releases = [{"tag_name": "v0.1.0", "draft": False}, {"tag_name": "v0.1.1", "draft": True}]
        self.assertEqual(release_tag(releases, [], 42), ("v0.1.2", None))
        self.assertEqual(release_tag(releases, [{"name": "v0.1.10"}, {"name": "v1.0.0"}], 42), ("v0.1.11", None))

    def test_retry_reuses_run_version(self) -> None:
        release = {"tag_name": "v0.1.3", "body": "notes\n<!-- mars-ci-run:42 -->"}
        self.assertEqual(release_tag([release], [], 42), ("v0.1.3", release))
        self.assertEqual(release_tag([release], [], 43), ("v0.1.4", None))

    def test_pr_stays_draft_and_main_publishes_only_after_upload(self) -> None:
        release = {"id": 3, "html_url": "https://example.invalid/release"}
        for event in ["pull_request", "push"]:
            with self.subTest(event=event):
                run = self.run | {"event": event, "head_branch": "main", "html_url": "https://example.invalid/ci"}
                replies = [run] + ([[self.pull]] if event == "pull_request" else []) + [self.artifacts, [], [], release]
                with patch("release.api", side_effect=replies) as remote:
                    with patch("release.package", return_value=[Path("archive.tar.gz")]):
                        with patch("release.subprocess.run") as commands:
                            publish("owner/repo", 42, False)
                creation = remote.call_args_list[-1].args[2]
                self.assertTrue(creation["draft"])
                self.assertEqual(creation["target_commitish"], "a" * 40)
                calls = [call.args[0] for call in commands.call_args_list]
                self.assertEqual(calls[0][1:3], ["run", "download"])
                self.assertEqual(calls[1][1:3], ["release", "upload"])
                self.assertEqual(len(calls), 2 if event == "pull_request" else 3)
                if event == "push":
                    self.assertIn("draft=false", calls[2])

    def test_published_retry_does_not_replace_assets(self) -> None:
        release = {
            "tag_name": "v0.1.0",
            "draft": False,
            "target_commitish": "a" * 40,
            "body": "<!-- mars-ci-run:42 -->",
        }
        run = self.run | {"event": "push", "head_branch": "main"}
        with patch("release.api", side_effect=[run, self.artifacts, [release], []]):
            with patch("release.subprocess.run") as commands:
                publish("owner/repo", 42, False)
        commands.assert_not_called()

    def test_skipped_or_expired_build_has_no_release(self) -> None:
        run = self.run | {"event": "push", "head_branch": "main"}
        for artifacts in [{"artifacts": []}, {"artifacts": [{"name": "mars-linux-x86_64", "expired": True}]}]:
            with patch("release.api", side_effect=[run, artifacts]) as remote:
                with patch("release.subprocess.run") as commands:
                    publish("owner/repo", 42, False)
            self.assertEqual(remote.call_count, 2)
            commands.assert_not_called()

    def test_archives_and_checksums_are_reproducible(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for feature in ["cpu", "gpu"]:
                (directory / f"mars-{feature}").write_bytes(b"\x7fELF\x02\x01" + b"\x00" * 12 + b"\x3e\x00")
            assets = package(directory, "v0.1.0", self.run)
            before = [path.read_bytes() for path in assets]
            with tarfile.open(assets[0]) as bundle:
                self.assertEqual(bundle.getnames(), ["mars", "BUILD.json"])
                self.assertEqual(bundle.getmember("mars").mode, 0o755)
                metadata = json.load(bundle.extractfile("BUILD.json"))
                self.assertEqual(metadata, {"version": "0.1.0", "commit": "a" * 40, "run_id": 42, "features": "cpu"})
            for line, path in zip(assets[-1].read_text().splitlines(), assets):
                self.assertEqual(line, f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")
            self.assertEqual(before, [path.read_bytes() for path in package(directory, "v0.1.0", self.run)])

    def test_invalid_binaries_and_symlinks_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            with self.assertRaises(ValueError):
                package(directory, "v0.1.0", self.run)
            (directory / "mars-cpu").write_bytes(b"not an executable")
            with self.assertRaises(ValueError):
                package(directory, "v0.1.0", self.run)
            (directory / "mars-gpu").symlink_to(directory / "mars-cpu")
            (directory / "mars-cpu").unlink()
            (directory / "mars-cpu").symlink_to(directory / "mars-gpu")
            with self.assertRaises(ValueError):
                package(directory, "v0.1.0", self.run)


if __name__ == "__main__":
    unittest.main()
