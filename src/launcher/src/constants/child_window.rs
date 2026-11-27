//! The authored child window (the family's `28`): the names of its viewport and accessible node, where
//! a new one appears and the size of each of the launcher's surfaces.

/// A child window's viewport id is a hash of this and its key: one native window per key.
pub const VIEWPORT: &str = "nxrs-child-window";
/// Its accessible node is `<title> window`.
pub const NODE: &str = "window";
pub const POPUP_NODE: &str = "popup";
/// What a native window says of itself (the run check), where it was, whether it is shown.
pub const SEEN: &str = "child-window-seen";
pub const PLACE: &str = "child-window-place";
pub const SHOWN: &str = "child-window-shown";
pub const DRAG_ID: &str = "child-window-drag";
pub const CLOSE_ID: &str = "child-window-close";
pub const MAIN_FRAME: &str = "child-window-main-frame";
pub const CAPTION_STRIP: &str = "child-window-caption-strip";
pub const EMBEDDED: &str = "child-window-embedded";
/// A tool window appears this far in from the main window's corner.
pub const OFFSET: (f32, f32) = (64.0, 96.0);
pub const BORDER: f32 = 1.0;
/// A window opens no larger than this share of the main window (and the monitor less the margin).
pub const OPEN_SHARE: f32 = 0.8;
pub const SCREEN_MARGIN: (f32, f32) = (32.0, 96.0);
pub const SIZE: &str = "child-window-size";
pub const OPENED_AT: &str = "child-window-opened-size";
pub const GRIP: f32 = 5.0;
pub const GRIP_ID: &str = "child-window-grip";
/// The scrim over the main window behind a modal window: dark black at 0.55, light `#111827` at 0.35.
pub const SCRIM_DARK: [u8; 4] = [0, 0, 0, 140];
pub const SCRIM_LIGHT: [u8; 4] = [0x11, 0x18, 0x27, 89];
pub const SCRIM_ID: &str = "child-window-scrim";

/// The launcher's surfaces: (key, content width, body height, least width, least body), from the Qt
/// dialogs' sizes (Properties 670 x 400, Settings 690 x 440, DLC 680 x 400, Updates 700 x 430).
pub type Surface = (&'static str, f32, f32, f32, f32);
/// The rail work windows share one size (content width, body height, least width, least body).
const RAIL: (f32, f32, f32, f32) = (840.0, 440.0, 730.0, 360.0);
pub const PROPERTIES: Surface = ("properties", RAIL.0, RAIL.1, RAIL.2, RAIL.3);
pub const ADD_GAME: Surface = ("add-game", RAIL.0, RAIL.1, RAIL.2, RAIL.3);
pub const SETTINGS: Surface = ("settings", RAIL.0, RAIL.1, RAIL.2, RAIL.3);
pub const DLC: Surface = ("dlc", 680.0, 400.0, 520.0, 300.0);
pub const UPDATES: Surface = ("updates", 700.0, 430.0, 540.0, 320.0);
/// A message or a question: its width; its body is measured from its text.
pub const MESSAGE_WIDTH: f32 = 440.0;
pub const MESSAGE_LEAST: f32 = 340.0;
/// A question's tick box: the paragraph gap over it, and the height it adds to the box.
pub const MESSAGE_CHECK_GAP: f32 = 12.0;
pub const MESSAGE_CHECK_ROW: f32 = 36.0;
pub const MESSAGE_KEY: &str = "message";
/// The Native / Web chooser: its width, its body (the Group heading, two choice tiles, the footer),
/// a tile's height.
pub const TARGET_WIDTH: f32 = 420.0;
pub const TARGET_BODY: f32 = 228.0;
pub const TARGET_TILE: f32 = 54.0;
