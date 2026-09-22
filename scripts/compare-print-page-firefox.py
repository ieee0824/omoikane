#!/usr/bin/env python3
"""Compare Omoikane's fixed print fixture with Firefox's printed PDF."""

from __future__ import annotations

import argparse
import base64
import json
from pathlib import Path
import subprocess


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--geckodriver", required=True, type=Path)
    parser.add_argument("--actual-dir", required=True, type=Path)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()

    from PIL import Image, ImageChops
    from selenium import webdriver
    from selenium.webdriver.common.print_page_options import PrintOptions
    from selenium.webdriver.firefox.options import Options
    from selenium.webdriver.firefox.service import Service

    fixture = (
        Path(__file__).resolve().parents[1]
        / "tests/fixtures/print/page-layers.html"
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    options = Options()
    options.add_argument("-headless")
    options.set_preference("print.print_bgcolor", True)
    options.set_preference("print.print_bgimages", True)
    service = Service(
        executable_path=str(args.geckodriver),
        log_path=str(args.output_dir / "geckodriver.log"),
    )
    print_options = PrintOptions()
    print_options.background = True
    print_options.page_width = 3.175  # 120 CSS px at 96 dpi, in cm
    print_options.page_height = 3.175
    print_options.margin_top = 0
    print_options.margin_right = 0
    print_options.margin_bottom = 0
    print_options.margin_left = 0
    print_options.shrink_to_fit = False
    with webdriver.Firefox(service=service, options=options) as browser:
        browser.get(fixture.as_uri())
        pdf = base64.b64decode(browser.print_page(print_options))
    (args.output_dir / "firefox.pdf").write_bytes(pdf)
    subprocess.run(
        [
            "pdftoppm",
            "-png",
            "-r",
            "96",
            str(args.output_dir / "firefox.pdf"),
            str(args.output_dir / "firefox"),
        ],
        check=True,
    )

    reference_pages = sorted(args.output_dir.glob("firefox-*.png"))
    actual_pages = sorted(args.actual_dir.glob("page-*.png"))
    results = []
    for index, (actual_path, reference_path) in enumerate(
        zip(actual_pages, reference_pages, strict=True), start=1
    ):
        with Image.open(actual_path) as actual_image, Image.open(
            reference_path
        ) as reference_image:
            actual = actual_image.convert("RGB")
            reference = reference_image.convert("RGB")
            if actual.size != reference.size:
                raise AssertionError(
                    f"page {index}: dimensions {actual.size} != {reference.size}"
                )
            diff = ImageChops.difference(actual, reference)
            changed_pixels = sum(pixel != (0, 0, 0) for pixel in diff.getdata())
            results.append(
                {
                    "page": index,
                    "size": actual.size,
                    "changed_pixels": changed_pixels,
                    "diff_bbox": diff.getbbox(),
                }
            )
    print(json.dumps({"pages": results}, sort_keys=True))
    return int(not results or any(page["changed_pixels"] for page in results))


if __name__ == "__main__":
    raise SystemExit(main())
