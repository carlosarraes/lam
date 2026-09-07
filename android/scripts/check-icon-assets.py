#!/usr/bin/env python3
"""Check the approved icon artwork and render Android mask/notification previews.

Requires rsvg-convert and ImageMagick. Run from any directory. Use
--generate-legacy after editing the canonical SVG and Android vectors, then
review the previews before committing the regenerated density assets.
"""

import argparse
from pathlib import Path
import re
import subprocess
import tempfile
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parents[2]
ANDROID = "{http://schemas.android.com/apk/res/android}"
SVG = "http://www.w3.org/2000/svg"
DENSITIES = {"mdpi": 48, "hdpi": 72, "xhdpi": 96, "xxhdpi": 144, "xxxhdpi": 192}
APPROVED = '''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128">
<rect width="128" height="128" rx="29" fill="#D5A64D"/>
<path d="M34 28v62c0 8 4 11 12 11" fill="none" stroke="#24272E" stroke-width="10" stroke-linecap="round"/>
<circle cx="74" cy="67" r="23" fill="none" stroke="#24272E" stroke-width="10"/>
<circle cx="74" cy="67" r="6" fill="#24272E"/>
</svg>'''


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def read_xml(path):
    require(path.is_file(), f"Missing icon asset: {path.relative_to(ROOT)}")
    return ET.parse(path).getroot()


def render(svg, size):
    png = subprocess.run(
        ["rsvg-convert", "-w", str(size), "-h", str(size)],
        input=svg.encode(), capture_output=True, check=True,
    ).stdout
    rgba = subprocess.run(
        ["magick", "png:-", "-depth", "8", "rgba:-"],
        input=png, capture_output=True, check=True,
    ).stdout
    return png, rgba


def similar(actual, expected, label):
    require(len(actual) == len(expected), f"{label}: wrong dimensions")
    error = sum(abs(a - b) for a, b in zip(actual, expected)) / len(actual) / 255
    require(error < 0.00005, f"{label}: artwork differs, mean pixel error {error:.5f}")


def vector_svg(path):
    """Translate the vector subset used here; reject unsupported drawing attributes."""
    vector = read_xml(path)
    require(vector.tag == "vector", f"{path.name}: expected a vector")
    width = vector.attrib[ANDROID + "viewportWidth"]
    height = vector.attrib[ANDROID + "viewportHeight"]

    def convert(node):
        attributes = {key.removeprefix(ANDROID): value for key, value in node.attrib.items()}
        if node.tag == "group":
            require(set(attributes) <= {"scaleX", "scaleY", "translateX", "translateY"},
                    f"{path.name}: unsupported group attributes")
            group = ET.Element("g", {"transform":
                f"translate({attributes.get('translateX', '0')} {attributes.get('translateY', '0')}) "
                f"scale({attributes.get('scaleX', '1')} {attributes.get('scaleY', '1')})"})
            group.extend(convert(child) for child in node)
            return group
        require(node.tag == "path", f"{path.name}: unsupported vector element {node.tag}")
        mapping = {"pathData": "d", "fillColor": "fill", "strokeColor": "stroke",
                   "strokeWidth": "stroke-width", "strokeLineCap": "stroke-linecap"}
        require(set(attributes) <= set(mapping), f"{path.name}: unsupported path attributes")
        require(bool(attributes.get("pathData")), f"{path.name}: empty path")
        result = ET.Element("path", {"fill": "none"})
        for key, value in attributes.items():
            result.set(mapping[key], "none" if value == "@android:color/transparent" else value)
        return result

    svg = ET.Element("svg", {"xmlns": SVG, "viewBox": f"0 0 {width} {height}"})
    svg.extend(convert(child) for child in vector)
    require(len(svg) > 0, f"{path.name}: empty vector")
    return svg


def serialize(svg):
    return ET.tostring(svg, encoding="unicode")


def masked(foreground, shape, background, tint=None):
    masks = {
        "circle": '<circle cx="54" cy="54" r="36"/>',
        "squircle": '<path d="M54 18C84 18 90 24 90 54S84 90 54 90S18 84 18 54S24 18 54 18Z"/>',
        "rounded-square": '<rect x="18" y="18" width="72" height="72" rx="16.3125"/>',
    }
    artwork = "".join(ET.tostring(child, encoding="unicode") for child in foreground)
    if tint:
        artwork = artwork.replace("#FFFFFF", tint)
    return (f'<svg xmlns="{SVG}" viewBox="18 18 72 72">'
            f'<defs><clipPath id="mask">{masks[shape]}</clipPath></defs>'
            f'<g clip-path="url(#mask)"><rect width="108" height="108" fill="{background}"/>'
            f'{artwork}</g></svg>')


def check_apk(apk, aapt2):
    dump = subprocess.run([aapt2, "dump", "resources", str(apk)],
                          capture_output=True, text=True, check=True).stdout
    resources = {name: (identifier, contents) for identifier, name, contents in re.findall(
        r"^    resource (0x[0-9a-f]+) (\S+)\n(.*?)(?=^    resource |\Z)", dump, re.M | re.S)}
    for name in ["drawable/ic_launcher_foreground", "drawable/ic_launcher_monochrome",
                 "drawable/ic_notification_lam", "color/ic_launcher_background"]:
        require(name in resources, f"APK is missing {name}")
    for name in ["mipmap/ic_launcher", "mipmap/ic_launcher_round"]:
        require(name in resources, f"APK is missing {name}")
        match = re.search(r"\(anydpi[^)]*\) \(file\) (\S+) type=XML", resources[name][1])
        require(match is not None, f"APK is missing the adaptive XML variant of {name}")
        xml = subprocess.run([aapt2, "dump", "xmltree", str(apk), "--file", match[1]],
                             capture_output=True, text=True, check=True).stdout
        require("E: adaptive-icon" in xml, f"APK {name} is not an adaptive icon")
        for tag, reference in [("background", "color/ic_launcher_background"),
                               ("foreground", "drawable/ic_launcher_foreground"),
                               ("monochrome", "drawable/ic_launcher_monochrome")]:
            child = re.search(rf"E: {tag}\b[^\n]*\n([^\n]+)", xml)
            require(child is not None and resources[reference][0] in child[1],
                    f"APK {name} has a broken {tag} reference")
    print(f"PASS: APK adaptive XML variants and references, themed and notification resources: {apk}")


def check(output, generate_legacy):
    res = ROOT / "android/app/src/main/res"
    canonical = read_xml(ROOT / "design/app-icon/lam-mark.svg")
    require(not canonical.findall(f".//{{{SVG}}}text"), "Canonical icon must not depend on fonts")
    similar(render(serialize(canonical), 384)[1], render(APPROVED, 384)[1], "Approved design")

    colors = {node.attrib["name"]: node.text for node in read_xml(res / "values/colors.xml")}
    require(colors.get("ic_launcher_background") == "#D5A64D", "Launcher background must use approved amber")
    app = read_xml(ROOT / "android/app/src/main/AndroidManifest.xml").find("application")
    require(app is not None, "Manifest has no application")
    for attribute, name in [("icon", "ic_launcher"), ("roundIcon", "ic_launcher_round")]:
        require(app.get(ANDROID + attribute) == f"@mipmap/{name}", f"Manifest {attribute} does not resolve to {name}")
        adaptive = read_xml(res / f"mipmap-anydpi/{name}.xml")
        require(adaptive.tag == "adaptive-icon", f"{name}: expected adaptive icon")
        for tag, reference in [("background", "@color/ic_launcher_background"),
                               ("foreground", "@drawable/ic_launcher_foreground"),
                               ("monochrome", "@drawable/ic_launcher_monochrome")]:
            node = adaptive.find(tag)
            require(node is not None and node.get(ANDROID + "drawable") == reference,
                    f"{name}: missing or wrong {tag} resource")

    foreground = vector_svg(res / "drawable/ic_launcher_foreground.xml")
    monochrome = vector_svg(res / "drawable/ic_launcher_monochrome.xml")
    notification = vector_svg(res / "drawable/ic_notification_lam.xml")
    reference = ET.fromstring(APPROVED)
    reference.remove(reference[0])
    expected = ET.Element("svg", {"xmlns": SVG, "viewBox": "0 0 108 108"})
    group = ET.SubElement(expected, "g", {"transform": "translate(14.7 15.3) scale(0.6)"})
    group.extend(reference)
    pixels = render(serialize(foreground), 324)[1]
    similar(pixels, render(serialize(expected), 324)[1], "Adaptive foreground")
    alpha = pixels[3::4]
    require(any(alpha), "Adaptive foreground is empty")
    for i, value in enumerate(alpha):
        if value:
            require((i % 324 + 0.5 - 162) ** 2 + (i // 324 + 0.5 - 162) ** 2 <= 99 ** 2,
                    "Adaptive artwork escapes the central 66 dp safe circle")
    mono_pixels = render(serialize(monochrome), 324)[1]
    similar(mono_pixels[3::4], alpha, "Themed silhouette")
    require(all(mono_pixels[i:i + 3] == b"\xff\xff\xff" for i in range(0, len(mono_pixels), 4)
                if mono_pixels[i + 3]), "Themed icon must be a white transparent silhouette")

    output.mkdir(parents=True, exist_ok=True)
    for shape in ["circle", "squircle", "rounded-square"]:
        png, _ = render(masked(foreground, shape, colors["ic_launcher_background"]), 432)
        (output / f"{shape}.png").write_bytes(png)
    (output / "themed.png").write_bytes(render(masked(monochrome, "circle", "#E8DDF8", "#493D5E"), 432)[0])
    notification_png, notification_pixels = render(serialize(notification), 24)
    # The ring stays open at small-icon size, with a separate visible attention dot.
    notification_alpha = notification_pixels[3::4]
    require(notification_alpha[13 * 24 + 14] > 220, "Notification attention dot disappears at 24 dp")
    require(notification_alpha[13 * 24 + 17] < 20, "Notification ring closes at 24 dp")
    require(notification_alpha[12 * 24 + 4] > 220, "Notification stem disappears at 24 dp")
    require(100 < sum(value > 128 for value in notification_alpha) < 250, "Notification has wrong ink coverage")
    require(all(notification_pixels[i:i + 3] == b"\xff\xff\xff" for i in range(0, len(notification_pixels), 4)
                if notification_pixels[i + 3]), "Notification must be white on transparency")
    require(all(notification_alpha[y * 24 + x] == 0 for y in range(24) for x in range(24)
                if x in (0, 23) or y in (0, 23)), "Notification touches its 24 dp boundary")
    (output / "notification-24dp.png").write_bytes(notification_png)
    keep = read_xml(res / "raw/keep.xml")
    require("@drawable/ic_notification_lam" in keep.get("{http://schemas.android.com/tools}keep", "").split(","),
            "Release shrinking must retain the notification resource")

    for density, size in DENSITIES.items():
        for name, shape in [("ic_launcher", "rounded-square"), ("ic_launcher_round", "circle")]:
            path = res / f"mipmap-{density}/{name}.png"
            expected_png, expected_pixels = render(masked(foreground, shape, colors["ic_launcher_background"]), size)
            if generate_legacy:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(expected_png)
            require(path.is_file(), f"Missing density icon: {path.relative_to(ROOT)}")
            actual_pixels = subprocess.run(["magick", str(path), "-depth", "8", "rgba:-"],
                                           capture_output=True, check=True).stdout
            similar(actual_pixels, expected_pixels, f"{density}/{name}")
    print("PASS: approved artwork, manifest links, adaptive safe zone, themed silhouette, 24 dp notification, 10 density assets")
    print(f"Previews: {output}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--previews", type=Path)
    parser.add_argument("--generate-legacy", action="store_true")
    parser.add_argument("--apk", type=Path, help="Also inspect a built APK for retained adaptive and notification resources")
    parser.add_argument("--aapt2", default="aapt2", help="Android SDK aapt2 executable")
    args = parser.parse_args()
    try:
        if args.previews:
            check(args.previews.resolve(), args.generate_legacy)
        else:
            with tempfile.TemporaryDirectory(prefix="lam-icon-previews-") as directory:
                check(Path(directory), args.generate_legacy)
        if args.apk:
            check_apk(args.apk, args.aapt2)
    except (AssertionError, ET.ParseError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"FAIL: {error}\n")
