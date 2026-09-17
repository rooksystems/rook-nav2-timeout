# README diagrams

The diagrams use the visual style of [Rook's website](https://rookreplay.com), with Instrument Serif headings, Instrument Sans prose, Geist Mono evidence labels, a shared paper or charcoal background, and small orange cells. Color draws attention to the cancellation request. PASS and FAIL remain explicit text.

The desktop diagram shows selected events from the [recorded result](../result.md). Distances between marks do not encode elapsed time. The phone version keeps the same outcome and limits in a vertical layout. Both images adapt to the viewer's light or dark color scheme.

## Edit the images

Edit the text and geometry in [source/timeout.svg](source/timeout.svg) and [source/timeout-mobile.svg](source/timeout-mobile.svg). The published images contain glyph outlines so GitHub renders the house fonts without loading fonts from another server. Their accessible descriptions remain text, and the README supplies alternative text.

To regenerate them, install `fonttools[woff]==4.61.1` in a Python environment and put these font files in a directory of your choice.

- [InstrumentSerif-Regular.woff2](https://rookreplay.com/fonts/InstrumentSerif-Regular.woff2)
- [InstrumentSans-Variable.woff2](https://rookreplay.com/fonts/InstrumentSans-Variable.woff2)
- [GeistMono-Regular.woff2](https://rookreplay.com/fonts/GeistMono-Regular.woff2)

From the repository root, run the following command, replacing `/path/to/fonts` with that directory.

```sh
python3 tools/outline_diagrams.py /path/to/fonts
```

Inspect both sizes and themes after editing. The font software is not included in this repository. Instrument Serif and Instrument Sans are by the [Instrument project](https://github.com/Instrument); Geist Mono is by [Vercel](https://github.com/vercel/geist-font). All three use the SIL Open Font License.
