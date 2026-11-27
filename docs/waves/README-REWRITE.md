# README-REWRITE - a shorter README with a banner (2026-11-27)

Branch `readme-rewrite` from `gameplay-harness` (f18c09d). Docs and images only; no code touched.

## Plan

- Banner `docs/images/readme/banner.png` (1280x400) and `social-preview.png` (1280x640): the launcher's app icon
  (`src/launcher/assets/app.png`) and the name NXRS in the title bar's wordmark style, on the launcher's dark
  canvas colour. Nothing else.
- README.md rewritten to the owner-approved order, modelled on electron-markdownify (centred header, badges, nav row)
  and sniffnet (banner linking to the website, website badge, OS download table).
- Preview the rendered README and fix layout issues.

## Built

- `scripts/readme/banner.py` draws both images. It reads the word, size and letter spacing from
  `src/launcher/src/constants/layout.rs` (`WORDMARK`, `WORDMARK_SIZE`, `WORDMARK_SPACING`: 1.2 / 12 = 0.1 em)
  and the dark `CANVAS` (#111312) and `TEXT` (#f1f5f2) tokens from `src/launcher/src/constants/theme.rs`, so the
  banner follows the app. Font: Consolas Bold (`consolab.ttf`, the launcher's mono-bold face on Windows),
  DejaVu Sans Mono Bold on Linux. Sizes: banner.png 22 KB, social-preview.png 27 KB.
  Run with `python scripts/readme/banner.py`: the `py` launcher's 3.12 has no Pillow (no pip) on this machine, the
  pyenv `python` (3.12.5) has Pillow 12.3.
- README.md: 90 -> 73 lines, about half the text. Order: banner (links to nxrs.app), the one line, badges
  (Download -> latest release, Website -> nxrs.app, CI, License MIT; the two static badges in the launcher's lime
  accent #b5f36b), nav row, the commented-out hero.gif placeholder, the IMPORTANT note, the four key features
  verbatim, Install (requirements line, Windows / Linux table with direct links to the release assets
  `nxrs-windows-x64.zip` and `nxrs-linux-x64.tar.gz`, both present in the current release, and what else to
  install), Getting started (four steps), Build from source (unchanged command block), Credits ending with the
  nxrs.app line. Removed: the library screenshot, the Games grid, the in-game key list, "Play in the browser",
  the Launcher workflow badge. No browser named.
- Left in place: `docs/images/readme/library.png` and `docs/images/readme/games/` (no longer referenced by the
  README; deleting them is the lead's call).

## Verified

- Read both PNGs: icon left, NXRS right, centred on the dark canvas, nothing else.
- Rendered README.md with `gh api markdown` (gfm, repo context) into GitHub's dark markdown CSS at 1012 px content
  width and read a headless Chrome screenshot. Found and fixed: the Windows row wrapped "Windows 10 / 11" and the
  long asset filename; the table now shows `x64 (.zip)` / `x64 (.tar.gz)` (sniffnet style) and fits on one line per
  row. The IMPORTANT alert, badges, nav anchors (`#install`, `#getting-started`, `#build-from-source`, `#credits`)
  render correctly.

## Unproven

- The banner's #111312 shows as a slightly lighter box on GitHub's dark page (#0d1117) and a dark card on the light
  page; this is the launcher's canvas colour as asked.
- `social-preview.png` must be uploaded by hand in the repo's Settings > Social preview (not done; no repo settings
  changed).

## Slips

- One `sed -i` on README.md to change the badge hex from a guessed #b5f26b to the theme's #b5f36b (§4: editor tools
  only). Single line, two occurrences; the final diff of README.md shows it.
