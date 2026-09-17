#!/usr/bin/env python3
"""Outline the README diagrams so GitHub needs no external font requests.

Requires fonttools[woff]. Pass a directory containing the three named WOFF2
files. Editable text and geometry remain in docs/assets/source.
"""
import argparse
from pathlib import Path
import xml.etree.ElementTree as ET

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

SVG = 'http://www.w3.org/2000/svg'
ET.register_namespace('', SVG)
FONTS = {
    'Instrument Serif': 'InstrumentSerif-Regular.woff2',
    'Instrument Sans': 'InstrumentSans-Variable.woff2',
    'Geist Mono': 'GeistMono-Regular.woff2',
}


def outline(source, destination, fonts):
    tree = ET.parse(source)
    root = tree.getroot()
    for node in list(root):
        if node.tag != f'{{{SVG}}}text':
            continue
        font = fonts[node.attrib['font-family']]
        glyphs = font.getGlyphSet()
        cmap = font.getBestCmap()
        scale = float(node.attrib['font-size']) / font['head'].unitsPerEm
        spacing = float(node.attrib.get('letter-spacing', 0)) / scale
        group = ET.Element(f'{{{SVG}}}g', {
            'class': node.attrib['class'],
            'aria-label': node.text,
            'transform': f'translate({node.attrib["x"]} {node.attrib["y"]}) scale({scale:g} {-scale:g})',
        })
        # Glyph outlines keep the house typography stable in GitHub's SVG
        # image sandbox. The source text and image description stay readable.
        pen = SVGPathPen(glyphs)
        advance = 0
        for character in node.text:
            glyph = glyphs[cmap[ord(character)]]
            glyph.draw(TransformPen(pen, (1, 0, 0, 1, advance, 0)))
            advance += glyph.width + spacing
        ET.SubElement(group, f'{{{SVG}}}path', {'d': pen.getCommands()})
        root.insert(list(root).index(node), group)
        root.remove(node)
    ET.indent(tree, space='  ')
    tree.write(destination, encoding='unicode', xml_declaration=False)
    with destination.open('a') as output:
        output.write('\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('font_directory', type=Path)
    args = parser.parse_args()
    fonts = {name: TTFont(args.font_directory / file) for name, file in FONTS.items()}
    assets = Path(__file__).resolve().parents[1] / 'docs/assets'
    for source in sorted((assets / 'source').glob('timeout*.svg')):
        outline(source, assets / source.name, fonts)
