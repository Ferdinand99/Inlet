# Test font for the Linux screenshots

`NotoSansJP-subset.ttf` is used only by `scripts/xvfb-screens.sh`. It is not compiled
into Inlet and is not in any release.

The Linux screenshot job photographs demo data that contains Japanese text. The app
finds fonts for such text on the machine it runs on, and GitHub's runner images change
their installed fonts without notice, so a new image moved the anti-aliasing of the
Japanese channel name and failed ten scenes. The script now gives the app a closed set
of fonts (this one, DejaVu and Noto Color Emoji) through `FONTCONFIG_FILE`, so the
result no longer depends on the image.

- Source: Noto Sans JP 2.004 (variable, weight axis kept), from
  https://github.com/google/fonts/tree/main/ofl/notosansjp (built from
  https://github.com/notofonts/noto-cjk). Licensed under the SIL Open Font License 1.1,
  see `OFL.txt` (copied unchanged from there).
- Subset (fonttools 4.56.0), keeping the glyphs the demo data and the scenes use:
  U+3000-303F, U+3040-309F (hiragana), U+30A0-30FF (katakana), U+FF00-FFEF, U+FE35
  and the kanji U+65E5, U+672C, U+8A9E:

  ```
  pyftsubset "NotoSansJP[wght].ttf" \
    --unicodes="U+3000-303F,U+3040-309F,U+30A0-30FF,U+FF00-FFEF,U+FE35,U+65E5,U+672C,U+8A9E" \
    --name-IDs="*" --notdef-outline --output-file=NotoSansJP-subset.ttf
  ```

- Add glyphs here (and retake the Linux baselines) when demo data gets new CJK text.
