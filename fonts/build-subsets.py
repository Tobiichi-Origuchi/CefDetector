#!/usr/bin/env python3

import argparse
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont


TITLE_TEXT = """正在全盘搜索 CEF 应用，请耐心等待...
这台电脑上已找到 0123456789 个 Chromium 内核的应用 (0123456789. BKMGT) - 搜索中...
搜索完成！这台电脑上总共有 0123456789 个 Chromium 内核的应用 (0123456789. BKMGT)
搜索完成！这台电脑上没有 Chromium 内核的应用
搜索失败：
Repo: github.com/Tobiichi-Origuchi/CefDetector (求个STAR!)"""
CARD_TEXT = " ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789.…"


def set_names(
    font: TTFont,
    *,
    family: str,
    style: str,
    copyright_notice: str,
    license_text: str | None = None,
    license_url: str | None = None,
    trademark: str | None = None,
) -> None:
    names = font["name"]
    names.names.clear()
    postscript_family = family.replace(" ", "")
    values = {
        0: copyright_notice,
        1: family,
        2: style,
        3: f"{family} {style}",
        4: f"{family} {style}",
        5: "Version 1.000",
        6: f"{postscript_family}-{style}",
    }
    if trademark is not None:
        values[7] = trademark
    if license_text is not None:
        values[13] = license_text
    if license_url is not None:
        values[14] = license_url

    for name_id, value in values.items():
        names.setName(value, name_id, 3, 1, 0x409)


def subset_font(font: TTFont, text: str, *, keep_kerning: bool) -> None:
    options = subset.Options()
    options.hinting = False
    options.layout_features = ["kern"] if keep_kerning else []
    if not keep_kerning:
        options.drop_tables.extend(["GDEF", "GPOS", "GSUB", "vhea", "vmtx"])
    subsetter = subset.Subsetter(options=options)
    subsetter.populate(text=text)
    subsetter.subset(font)


def build_title(source: Path, destination: Path) -> None:
    font = TTFont(source)
    font.recalcTimestamp = False
    subset_font(font, TITLE_TEXT, keep_kerning=False)
    set_names(
        font,
        family="CefDetector Title",
        style="Heavy",
        copyright_notice=(
            "(c) Copyright Beijing HANYI KEYIN Information Technology Co."
        ),
        trademark="Trademark of HANYI",
    )
    font.save(destination)


def build_card(source: Path, destination: Path, weight: int, style: str) -> None:
    variable_font = TTFont(source)
    variable_font.recalcTimestamp = False
    font = instantiateVariableFont(
        variable_font,
        {"opsz": 14, "wght": weight},
        inplace=False,
        updateFontNames=True,
    )
    font.recalcTimestamp = False
    subset_font(font, CARD_TEXT, keep_kerning=True)
    set_names(
        font,
        family="CefDetector Card",
        style=style,
        copyright_notice=(
            "Copyright 2020 The Inter Project Authors (https://github.com/rsms/inter)"
        ),
        license_text="Licensed under the SIL Open Font License, Version 1.1.",
        license_url="https://openfontlicense.org",
    )
    font.save(destination)


def main() -> None:
    parser = argparse.ArgumentParser(description="Build CefDetector's embedded font subsets")
    parser.add_argument("--title-source", type=Path, required=True)
    parser.add_argument("--inter-source", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path(__file__).parent)
    args = parser.parse_args()

    args.output.mkdir(parents=True, exist_ok=True)
    build_title(args.title_source, args.output / "title-subset.ttf")
    build_card(args.inter_source, args.output / "card-regular-subset.ttf", 400, "Regular")
    build_card(args.inter_source, args.output / "card-bold-subset.ttf", 700, "Bold")


if __name__ == "__main__":
    main()
