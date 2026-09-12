# Box shadow and rounded boundary fixture (#676)

This fixed input separates CSS `box-shadow` blur radii of 1, 2, 4, 8, and
16px, a zero-blur control, positive and negative spread, a rounded shadow, and
rounded borders with 4px, 16px, and pill radii. Element geometry is fixed so
raster differences can be measured independently from layout.

The previous painter removed the border-box interior from the alpha mask before
blurring it and applied three box filters whose radius equaled the CSS blur
value. CSS Backgrounds requires the opaque shadow shape to be blurred first,
with a Gaussian standard deviation equal to half the CSS blur radius, and the
finished outer shadow to be clipped inside the border edge. The corrected path
uses a bounded direct Gaussian for 1px < blur <= 4px and three linear-time box
passes for other values, allocates the full filter support, then applies a
rounded coverage knockout. Rounded backgrounds, images, and borders use eight
vertical samples with analytic horizontal coverage at boundary pixels; fully
covered scanline spans are still painted in bulk.

Specification:

- https://drafts.csswg.org/css-backgrounds-3/#shadow-shape
- https://drafts.csswg.org/css-backgrounds-3/#shadow-blur

## Verification

Linux ARM64, Firefox 155.0.1, geckodriver 0.37.1, DPR 1, and fixed 800x600 and
700x450 viewports were used on 2026-09-12. Both browsers reproduced identical
pixels across their own repeated captures. All probed element rectangles
matched exactly.

For the original `8px 8px 8px #0006` case, the shadow region improved from
5,168 differing pixels, maximum channel error 74/255 and normalized MAE
0.0218553 to 4,024 pixels, maximum 5/255 and MAE 0.00110876. The rounded-border
region changed from 267 pixels, maximum 140/255 and MAE 0.00218048 to 293
pixels, maximum 118/255 and MAE 0.00157813. The extra changed edge pixels are
partial coverage; maximum and total error decreased. Overflow clipping and
translation controls remain pixel-identical.

In the matrix, maximum per-channel shadow errors for 1/2/4/8/16px blur were
4/2/5/5/3. Zero blur matched exactly; positive and negative spread each had a
maximum error of 5. Rounded shadow residuals were confined to antialiased corner
pixels. The Firefox/Omoikane composite was visually reviewed: blur extent,
strength, offsets, spreads, corner shapes, and element positions match.

Raw captures, repeats, losslessly compressed copies, intermediate algorithm
comparisons, and numeric logs are retained under
`/workspace/.artifacts/issues676/`. The final original fixture is in `final/`
and the final matrix is in `matrix-final/`.
