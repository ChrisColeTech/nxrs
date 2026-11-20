//! Where the title bar's parts are, in physical client pixels, and what a point there is.

use windows_sys::Win32::UI::WindowsAndMessaging::{HTCAPTION, HTCLIENT, HTMAXBUTTON};

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct PxRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}
impl PxRect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
    /// The whole column above the button's bottom: the screen-edge pixel of a maximized window hits it.
    fn column(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && x < self.x1 && y < self.y1
    }
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Regions {
    /// the bar's bottom edge
    pub bar_bottom: i32,
    pub minimize: PxRect,
    pub maximize: PxRect,
    pub close: PxRect,
    /// menu titles and anything else in the bar that takes clicks
    pub client: Vec<PxRect>,
}

/// `TitleBar::nativeHitCode`: caption (drag, double-click, snap), the maximize button (Snap
/// Layouts), or client (menus, the other buttons, everything under the bar).
pub fn hit_code(r: &Regions, x: i32, y: i32) -> u32 {
    if y >= r.bar_bottom {
        return HTCLIENT;
    }
    if r.maximize.column(x, y) {
        return HTMAXBUTTON;
    }
    if r.minimize.column(x, y) || r.close.column(x, y) || r.client.iter().any(|c| c.contains(x, y))
    {
        return HTCLIENT;
    }
    HTCAPTION
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn title_bar_hit_regions() {
        let r = Regions {
            bar_bottom: 108,
            minimize: PxRect {
                x0: 2000,
                y0: 0,
                x1: 2138,
                y1: 108,
            },
            maximize: PxRect {
                x0: 2138,
                y0: 0,
                x1: 2276,
                y1: 108,
            },
            close: PxRect {
                x0: 2276,
                y0: 0,
                x1: 2414,
                y1: 108,
            },
            client: vec![PxRect {
                x0: 93,
                y0: 10,
                x1: 200,
                y1: 100,
            }],
        };
        assert_eq!(hit_code(&r, 2050, 50), HTCLIENT);
        assert_eq!(hit_code(&r, 2200, 50), HTMAXBUTTON);
        assert_eq!(hit_code(&r, 2200, 0), HTMAXBUTTON);
        assert_eq!(hit_code(&r, 2300, 50), HTCLIENT);
        assert_eq!(hit_code(&r, 120, 50), HTCLIENT);
        assert_eq!(hit_code(&r, 30, 50), HTCAPTION);
        assert_eq!(hit_code(&r, 1000, 50), HTCAPTION);
        assert_eq!(hit_code(&r, 1000, 120), HTCLIENT);
        assert_eq!((HTCAPTION, HTMAXBUTTON, HTCLIENT), (2, 9, 1));
    }
}
