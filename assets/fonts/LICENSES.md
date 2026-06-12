# Bundled fonts

MAE bundles **JetBrains Mono**, patched with [Nerd Fonts](https://github.com/ryanoasis/nerd-fonts)
glyphs for icon support. Every component is under a permissive, GPL-compatible
license. The patch **deliberately excludes the "Font Logos" set** (third-party
brand/distro logos), which is unlicensed and trademark-encumbered.

## Base font

| Font | License |
|------|---------|
| JetBrains Mono | SIL Open Font License 1.1 (`OFL.txt`) |

## Bundled glyph sets (all permissive)

| Glyph set | License |
|-----------|---------|
| Seti-UI + Custom | MIT |
| Devicons | MIT |
| Octicons | MIT |
| Font Awesome (icons) | CC BY 4.0 |
| Font Awesome Extension | MIT |
| Material Design Icons | Apache-2.0 |
| Weather Icons | SIL OFL 1.1 |
| Codicons | CC BY 4.0 |
| Pomicons | SIL OFL 1.1 |
| Powerline Symbols | MIT |
| Powerline Extra Symbols | MIT |
| IEC Power Symbols | MIT |

**Excluded:** Font Logos (unlicensed brand/distro logos — trademark concern).

## Reproducing these files

Built from JetBrains Mono **v2.304** static TTFs with the Nerd Fonts
`font-patcher` (v3.4.0). Glyph flags include every permissive set and omit
`--fontlogos`:

```sh
# Tools: fontforge + Nerd Fonts FontPatcher.zip
for s in Regular Bold Italic BoldItalic; do
  fontforge -script font-patcher --mono \
    --codicons --fontawesome --fontawesomeext --material --octicons \
    --pomicons --powerline --powerlineextra --powersymbols --weather \
    --outputdir out JetBrainsMono-$s.ttf
done
# then rename the long output names to JetBrainsMono-<Style>.ttf
```

(The Seti-UI/Custom, Devicons, and Powerline core glyphs are added by the
patcher's default set.)
