//! srv dispatch over the real IPC buffer, and the APT state machine

mod common;

use common::{process, reply_word, staged, write_request, TLS};
use os3ds::hle::{Request, Service, R_INVALID_CMD, R_NOT_FOUND, R_OK};
use os3ds::services::{apt, srv};
use os3ds::Host;

fn name_words(name: &str) -> [u32; 2] {
    let mut bytes = [0u8; 8];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    [
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        u32::from_le_bytes(bytes[4..].try_into().unwrap()),
    ]
}

fn request(cmd: u32, params: Vec<u32>) -> Request {
    Request { cmd, normal: params.len(), translate: 0, params, translated: Vec::new() }
}

#[test]
fn get_service_handle_opens_a_session_through_the_ipc_buffer() {
    let host = Host::new(staged("srv"), Vec::new());
    let process = process();
    let srv_session = host.connect("srv:").expect("the srv: port");
    let [a, b] = name_words("hid:USER");
    // 3dbrew SRV:GetServiceHandle 0x00050100: name (2 words), length, flags
    write_request(&process, srv::GET_SERVICE_HANDLE, &[a, b, 8, 0], &[]);
    assert!(host.send_sync(&process, TLS, 0, 0, srv_session));
    assert_eq!(reply_word(&process, 1), R_OK);
    // header: command 5, 1 normal word (the result), 2 translate words
    assert_eq!(reply_word(&process, 0), (5 << 16) | (1 << 6) | 2);
    let handle = reply_word(&process, 3);
    assert_eq!(host.service_of(handle), Some(Service::Hid));
}

#[test]
fn an_unregistered_service_is_refused_and_ledgered() {
    let host = Host::new(staged("srv-missing"), Vec::new());
    let process = process();
    let srv_session = host.connect("srv:").unwrap();
    let [a, b] = name_words("xyz:u");
    write_request(&process, srv::GET_SERVICE_HANDLE, &[a, b, 5, 0], &[]);
    assert!(host.send_sync(&process, TLS, 0, 0, srv_session));
    assert_eq!(reply_word(&process, 1), R_NOT_FOUND);
    assert!(host.gaps().iter().any(|(line, _)| line.contains("xyz:u")));
    assert!(host.connect("nope:").is_none(), "an unknown port is not connected");
}

#[test]
fn enable_notification_hands_out_one_semaphore() {
    let host = Host::new(staged("srv-notify"), Vec::new());
    let process = process();
    let first = srv::command(&host, &process, TLS, 0, &request(srv::ENABLE_NOTIFICATION, vec![]));
    assert_eq!(first.words[0], R_OK);
    assert_eq!(first.translate, 2);
    assert_ne!(first.words[2], 0, "the notification semaphore");
    let again = srv::command(&host, &process, TLS, 0, &request(srv::ENABLE_NOTIFICATION, vec![]));
    assert_eq!(again.words[2], first.words[2]);
}

#[test]
fn an_unknown_srv_command_is_a_gap_not_a_success() {
    let host = Host::new(staged("srv-unknown"), Vec::new());
    let process = process();
    let reply = srv::command(&host, &process, TLS, 0, &request(0x0099, vec![]));
    assert_eq!(reply.words[0], R_INVALID_CMD);
    assert_eq!(host.gaps().len(), 1);
}

#[test]
fn the_apt_lifecycle_delivers_one_wakeup_parameter() {
    let host = Host::new(staged("apt"), Vec::new());
    let process = process();
    let init = apt::command(&host, &process, TLS, &request(apt::INITIALIZE, vec![0x300, 0]));
    assert_eq!(init.words[0], R_OK);
    assert_eq!(init.words[1], 0x0400_0000, "two copied handles");
    assert!(host.state.lock().unwrap().apt.initialized);
    assert!(host.state.lock().unwrap().apt_parameter_event.is_some());
    apt::command(&host, &process, TLS, &request(apt::ENABLE, vec![0]));
    assert!(host.state.lock().unwrap().apt.enabled);
    // Glance leaves the parameter pending, Receive consumes it
    let glance = apt::command(&host, &process, TLS, &request(apt::GLANCE_PARAMETER, vec![0x300, 0]));
    assert_eq!(glance.words[..3], [R_OK, 0, apt::SIGNAL_WAKEUP]);
    let receive = apt::command(&host, &process, TLS, &request(apt::RECEIVE_PARAMETER, vec![0x300, 0]));
    assert_eq!(receive.words[..3], [R_OK, 0, apt::SIGNAL_WAKEUP]);
    assert!(!host.state.lock().unwrap().apt.parameter_pending);
    let none = apt::command(&host, &process, TLS, &request(apt::RECEIVE_PARAMETER, vec![0x300, 0]));
    assert_eq!(none.words[0], 0xC8A0_CFFA, "no parameter pending");
    // the menu state the title reads
    let info = apt::command(&host, &process, TLS, &request(apt::GET_APPLET_MAN_INFO, vec![0]));
    assert_eq!(info.words, vec![R_OK, 0, 0x300, 0x101, 0x300]);
    let notification = apt::command(&host, &process, TLS, &request(apt::INQUIRE_NOTIFICATION, vec![0x300]));
    assert_eq!(notification.words, vec![R_OK, 0], "no notification pending");
}

#[test]
fn apt_utilities_without_an_applet_manager_are_ledgered() {
    let host = Host::new(staged("apt-utility"), Vec::new());
    let process = process();
    apt::command(&host, &process, TLS, &request(apt::APPLET_UTILITY, vec![7, 4, 4]));
    assert!(host.gaps().iter().any(|(line, _)| line.contains("AppletUtility 0x7")));
    let reset = apt::command(&host, &process, TLS, &request(apt::HARDWARE_RESET_ASYNC, vec![]));
    assert_eq!(reset.words[0], R_INVALID_CMD);
}
