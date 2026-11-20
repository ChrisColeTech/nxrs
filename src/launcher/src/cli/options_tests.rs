//! The flags parse as the Qt launcher's did.

use super::*;
#[test]
fn flags_parse_as_before() {
    let o = parse_from([
        "nxrs",
        "--library",
        "l.json",
        "--install-dlc",
        "a.nsp",
        "--install-dlc",
        "b.nsp",
        "--select-title",
        "Kart",
        "--screenshot-delay",
        "50",
    ])
    .unwrap();
    assert_eq!(o.library.as_deref(), Some("l.json"));
    assert_eq!(o.install_dlc, vec!["a.nsp".to_string(), "b.nsp".into()]);
    assert_eq!(o.screenshot_delay, 50);
    assert!(o.headless());
    let o = parse_from([
        "nxrs",
        "--demo",
        "--show-properties",
        "--properties-tab",
        "3",
    ])
    .unwrap();
    assert!(o.demo && o.show_properties && !o.headless());
    assert_eq!(o.properties_tab, 3);
    assert!(parse_from(["nxrs", "--bogus"]).is_err());
}
