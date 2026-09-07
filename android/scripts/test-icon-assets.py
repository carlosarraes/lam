#!/usr/bin/env python3
"""Prove the icon gate rejects missing, miswired, and visually broken assets."""

import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import tempfile
import unittest
import xml.etree.ElementTree as ET


SPEC = importlib.util.spec_from_file_location("icons", Path(__file__).with_name("check-icon-assets.py"))
icons = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(icons)
SOURCE = icons.ROOT


class IconGateTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="lam-icon-test-")
        self.addCleanup(self.directory.cleanup)
        icons.ROOT = Path(self.directory.name)
        self.addCleanup(setattr, icons, "ROOT", SOURCE)
        for name in ["design/app-icon", "android/app/src/main/res"]:
            shutil.copytree(SOURCE / name, icons.ROOT / name)
        shutil.copy2(SOURCE / "android/app/src/main/AndroidManifest.xml",
                     icons.ROOT / "android/app/src/main/AndroidManifest.xml")
        self.res = icons.ROOT / "android/app/src/main/res"

    def check(self):
        with contextlib.redirect_stdout(io.StringIO()):
            icons.check(icons.ROOT / "previews", False)

    def test_missing_canonical_is_rejected(self):
        (icons.ROOT / "design/app-icon/lam-mark.svg").unlink()
        with self.assertRaisesRegex(AssertionError, "Missing icon asset"):
            self.check()

    def test_missing_foreground_is_rejected(self):
        (self.res / "drawable/ic_launcher_foreground.xml").unlink()
        with self.assertRaisesRegex(AssertionError, "Missing icon asset"):
            self.check()

    def test_wrong_adaptive_reference_is_rejected(self):
        path = self.res / "mipmap-anydpi/ic_launcher.xml"
        tree = ET.parse(path)
        tree.getroot().find("foreground").set(icons.ANDROID + "drawable", "@drawable/missing")
        tree.write(path)
        with self.assertRaisesRegex(AssertionError, "wrong foreground resource"):
            self.check()

    def test_missing_foreground_dot_is_rejected(self):
        path = self.res / "drawable/ic_launcher_foreground.xml"
        tree = ET.parse(path)
        group = tree.getroot().find("group")
        group.remove(group[-1])
        tree.write(path)
        with self.assertRaisesRegex(AssertionError, "Adaptive foreground: artwork differs"):
            self.check()

    def test_wrong_density_artwork_is_rejected(self):
        path = self.res / "mipmap-mdpi/ic_launcher.png"
        shutil.copy2(self.res / "mipmap-mdpi/ic_launcher_round.png", path)
        with self.assertRaisesRegex(AssertionError, "mdpi/ic_launcher: artwork differs"):
            self.check()

    def test_missing_density_is_rejected(self):
        (self.res / "mipmap-xxxhdpi/ic_launcher_round.png").unlink()
        with self.assertRaisesRegex(AssertionError, "Missing density icon"):
            self.check()


if __name__ == "__main__":
    unittest.main()
