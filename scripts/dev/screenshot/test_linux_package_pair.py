"""Archive extraction must retain bundled links without escaping its root."""

import importlib.util
import io
import pathlib
import tarfile
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("package_pair", pathlib.Path(__file__).with_name("validate-linux-package-pair.py"))
package_pair = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_pair)


class BundleFilterTests(unittest.TestCase):
    def test_parent_relative_bundle_link_extracts_without_rewriting_it(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            archive_path = root / "bundle.tar"
            with tarfile.open(archive_path, "w") as archive:
                regular = tarfile.TarInfo("bundle/lib/loader")
                regular.size = 4
                archive.addfile(regular, io.BytesIO(b"data"))
                link = tarfile.TarInfo("bundle/lib64/loader")
                link.type = tarfile.SYMTYPE
                link.linkname = "../lib/loader"
                archive.addfile(link)
            destination = root / "extract"
            destination.mkdir()
            with tarfile.open(archive_path) as archive:
                archive.extractall(destination, filter=package_pair.bundle_filter)
            link = destination / "bundle/lib64/loader"
            self.assertEqual(link.readlink(), pathlib.Path("../lib/loader"))
            self.assertEqual(link.read_bytes(), b"data")

    def test_links_and_members_outside_destination_are_refused(self):
        with tempfile.TemporaryDirectory() as scratch:
            for name, target in [("bundle/link", "../../outside"), ("nested/link", "../../escape"), ("bundle/link", "/etc/passwd"), ("../outside", None)]:
                with self.subTest(name=name, target=target):
                    member = tarfile.TarInfo(name)
                    if target:
                        member.type = tarfile.SYMTYPE
                        member.linkname = target
                    with self.assertRaises(tarfile.FilterError):
                        package_pair.bundle_filter(member, scratch)

    def test_original_target_is_checked_before_modern_filter_can_accept_it(self):
        # The host may have the old 3.10 backport. Emulate modern symlink
        # resolution, including the former incorrectly rewritten target.
        with tempfile.TemporaryDirectory() as scratch:
            def modern_filter(member, destination):
                target = (pathlib.Path(destination) / pathlib.Path(member.name).parent / member.linkname).resolve()
                self.assertTrue(target.is_relative_to(pathlib.Path(destination)))
                return member

            with mock.patch.object(package_pair.tarfile, "data_filter", side_effect=modern_filter) as standard:
                member = tarfile.TarInfo("nested/link")
                member.type = tarfile.SYMTYPE
                member.linkname = "../../escape"
                with self.assertRaises(tarfile.LinkOutsideDestinationError):
                    package_pair.bundle_filter(member, scratch)
                standard.assert_not_called()
                member.linkname = "../inside"
                filtered = package_pair.bundle_filter(member, scratch)
                self.assertEqual(filtered.linkname, "../inside")
                standard.assert_called_once()

    def test_existing_link_cannot_redirect_extraction_outside_destination(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = pathlib.Path(scratch)
            destination = root / "extract"
            destination.mkdir()
            (destination / "redirect").symlink_to(root)
            with self.assertRaises(tarfile.FilterError):
                package_pair.bundle_filter(tarfile.TarInfo("redirect/outside"), str(destination))


if __name__ == "__main__":
    unittest.main()
