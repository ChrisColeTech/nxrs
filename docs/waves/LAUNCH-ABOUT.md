# LAUNCH-ABOUT: the About dialog's links, "NXRS" centred in the title bar

Branch `launch-about`, recreated from `gameplay-harness` 1162e39 (the first pass, 7eafb07, sat on the stale base
3d12ad0 with the pre-guide message box; the lead sent it back, and its work was re-applied onto the current
code). Launcher only (`src/launcher`); no titles, no GPU.

## Plan

1. **About** (`Help > About nxrs`) is a message per style guide §5 as the current code draws it
   (`ui/windows/message.rs`, the look of `docs/images/dialogs/error-add.png`): window title "About nxrs", the
   icon tile and heading "nxrs" (`page::heading`), the text, a footer with one filled OK last; OK, Enter, the X
   and Escape close it; no footer dismissal. Added:
   - the paragraph `Website: https://nxrs.app` in the text;
   - About's own verbs as ghost buttons at the footer's leading side (§5: "the surface's own verbs as ghost
     buttons at the leading side"; `authored::footer_bar`'s `lead`, as the list windows use it): **Website**
     (https://nxrs.app) and **GitHub** (https://github.com/ChrisColeTech/nxrs), opening the default browser
     through `services::shell_open`.
2. **Title bar**: `NXRS` at the centre of the window's full width, in the family's wordmark type (giga-cut
   `chrome/brand.rs`: mono bold 12 pt, 1.2 pt letter spacing, the text colour; the guide has no title-bar text
   rule, so this is the closest existing pattern). Not drawn when the centred name would come within 12 pt of the
   menu titles or the window buttons (never shifted, clipped or overlapping). It stays part of the drag area.

Files: `constants/{words_library,layout}.rs`, `flows/{action,dispatch,surface,start}.rs`,
`services/{shell_open,font_files}.rs`, `ui/{fonts,theme}.rs`, `ui/windows/{mod,message}.rs`,
`ui/chrome/{mod,title_bar}.rs`, new `ui/chrome/wordmark.rs`, `tests/ui_scaffold.rs`; captures in
`docs/images/dialogs/about.png` (replaced) and `docs/images/launch-about/`.

## Built

- **About.** `Surface::Message` gained `links` (label, web address), empty for every other message;
  `App::message_with_links` opens About with `words_library::ABOUT_LINKS`. `message::message` takes them and
  draws each as `button::ghost` in `authored::footer_bar`'s leading slot before the filled OK; a press answers
  `Action::OpenUrl(address)`, which `dispatch` hands to the new `shell_open::url` (`open::that_detached` with the
  address as given; `shell_open::open` rewrites slashes for paths, so it cannot take a URL). OK only closes.
- **Title bar.** `ui/chrome/wordmark.rs`, called from `title_bar::show` after the window buttons are laid out
  (the current title bar is unchanged from the old base in everything this touches). nxrs had no bold mono face,
  so `font_files`/`fonts`/`theme` add one (`consolab.ttf`; DejaVu Sans Mono Bold on Linux; falls back to the mono
  family). The name is centred by its ink box, not its line box: on the first pass the line box sat the capitals
  ~2 pt above the app glyph and menu titles (Consolas' descender room); now they are level. `wordmark::place`
  returns None when crowded. Hover-only sense, so the bar under it still drags (Windows' native hit test treats
  only menu titles as client). With `WINDOW_MIN` 920 pt the hide rule never triggers on the real window; it
  guards a smaller bar (tested at 420 pt).

## Tests

`tests/ui_scaffold.rs` (both fail on the base: no Website/GitHub buttons, no NXRS label):
- `about_shows_the_website_and_its_buttons_ask_for_the_right_pages`: the text contains the website; the URLs are
  exactly the owner's; with the dialogs drawn alone (`ui::windows::show`, no dispatch, so no browser opens),
  Website answers exactly `[OpenUrl("https://nxrs.app")]`, GitHub
  `[OpenUrl("https://github.com/ChrisColeTech/nxrs")]`, and About stays open; the header X `Close About nxrs`
  exists, no footer Close; OK answers nothing and closes About.
- `the_title_bar_centres_nxrs_and_drops_it_when_crowded`: the real shell at 1280 pt has the `NXRS` label centred at
  x = 640 (within 1 pt); at 420 pt it is absent; `place` centres, and refuses near the menus and near the window
  buttons.
- The base's `a_message_has_the_header_and_one_filled_ok` (About: title, OK, X, no footer dismissal) still passes.

Run at below-normal priority: `cargo test` all suites pass; `cargo clippy --all-targets -- -D warnings` clean;
`cargo fmt --check` clean. Files within 200/250 lines.

## Verified

Release `nxrs.exe` from this worktree, `NXRS_DATA_DIR` on a scratch copy of `%LOCALAPPDATA%\nxrs\library.json`,
PrintWindow at 3x, each run closed by its PID; captures read:
- `docs/images/launch-about/main-wide.png` (maximized, downscaled to 1600 wide) and `title-bar.png` (full-size
  crop): `NXRS` at the window's centre, letter-spaced bold mono, its capitals level with the app glyph.
- `docs/images/launch-about/main-narrow.png`: the window at its 920 pt minimum: `NXRS` centred between Help and
  the window buttons with room on both sides.
- `docs/images/launch-about/about.png` = `docs/images/dialogs/about.png` (`nxrs --show-window about`): window
  "About nxrs" with its X, the `i` tile and heading "nxrs", the text ending `Website: https://nxrs.app`, footer
  ghost Website and GitHub leading and the filled OK last; the same build as `error-add.png`.
The browser was never opened.

## Unproven

- `shell_open::url` launching the browser was not exercised (by design); it is the `open` crate call that
  `shell_open::open` already uses for folders.
- The Linux bold mono path is not run here.

## Slips

None.

## Questions

- The guide has no title-bar wordmark rule. Recommendation: one line in §5 ("the product name NXRS centred on the
  title bar in the family wordmark type; not drawn when crowded").
