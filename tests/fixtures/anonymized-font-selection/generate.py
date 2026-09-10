"""Generate original deterministic test fonts; requires fonttools==4.59.2."""
from pathlib import Path
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTCollection

root = Path(__file__).resolve().parent

def font(style, weight, italic, advance, width=5, family='Omoikane Fixture', fallback=False):
    builder = FontBuilder(1000, isTTF=True)
    names = ['.notdef', 'space', 'A', 'B']
    cmap = {32: 'space', 65: 'A', 66: 'B'}
    if fallback:
        names += ['acutecomb', 'CJK']
        cmap.update({0x0301: 'acutecomb', 0x4E2D: 'CJK'})
    builder.setupGlyphOrder(names)
    builder.setupCharacterMap(cmap)
    glyphs = {}
    for name in names:
        pen = TTGlyphPen(None)
        if name != 'space':
            skew = 80 if italic else 0
            pen.moveTo((50, 0))
            pen.lineTo((advance - 100, 0))
            pen.lineTo((advance - 100 + skew, 700))
            pen.lineTo((50 + skew, 700))
            pen.closePath()
        glyphs[name] = pen.glyph()
    builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (0 if name == 'acutecomb' else advance, 50) for name in names})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({
        'familyName': family, 'styleName': style,
        'typographicFamily': family, 'typographicSubfamily': style,
        'uniqueFontIdentifier': f'{family.replace(" ", "")}-{style}',
        'fullName': f'{family} {style}',
        'psName': f'{family.replace(" ", "")}-{style.replace(" ", "")}',
        'version': 'Version 1.0',
        'copyright': 'Original diagnostic outlines. CC0-1.0.',
    })
    selection = (1 if italic else 0) | (32 if weight >= 700 else 0)
    if not italic and weight == 400:
        selection |= 64
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200,
                    usWinAscent=800, usWinDescent=200, usWeightClass=weight,
                    usWidthClass=width, fsSelection=selection)
    builder.setupPost(italicAngle=-12 if italic else 0)
    builder.setupMaxp()
    builder.font['head'].macStyle = (1 if weight >= 700 else 0) | (2 if italic else 0)
    builder.font['head'].created = builder.font['head'].modified = 3660681600
    builder.font.recalcTimestamp = False
    return builder.font

specs = [('Regular', 400, False, 500, 5), ('Bold', 700, False, 700, 5),
         ('Italic', 400, True, 600, 5), ('BoldItalic', 700, True, 800, 5),
         ('Condensed', 400, False, 350, 3)]
fonts = {}
for style, weight, italic, advance, width in specs:
    fonts[style] = font(style, weight, italic, advance, width)
    fonts[style].save(root / f'OmoikaneFixture-{style}.ttf')
collection = TTCollection()
collection.fonts = [fonts['Bold'], fonts['Regular'], fonts['Italic']]
collection.save(root / 'OmoikaneFixture.ttc')
font('Regular', 400, False, 900, family='Omoikane Fallback', fallback=True).save(
    root / 'OmoikaneFallback-Regular.ttf')
