//! FSUSER over a staged tree: save-data create/open/write/delete/rename, the open flags, the
//! read-only RomFS (SelfNCCH) served from an image range

mod common;

use common::{process, staged, HEAP, TLS};
use os3ds::hle::{Request, Service, R_OK, R_PATH_NOT_FOUND};
use os3ds::services::fs;
use os3ds::Host;
use std::sync::Arc;

fn request(cmd: u32, params: Vec<u32>, translated: Vec<u32>) -> Request {
    Request { cmd, normal: params.len(), translate: translated.len(), params, translated }
}

/// OpenArchive(id) with an empty path; the archive handle
fn open_archive(host: &Host, process: &runtime3ds::Process3ds, id: u32) -> u32 {
    let reply = fs::command(host, process, TLS, &request(fs::OPEN_ARCHIVE, vec![id, 1, 0], vec![0, 0]));
    assert_eq!(reply.words[0], R_OK);
    reply.words[1]
}

/// an ASCII path at HEAP; (type, size, pointer)
fn path(process: &runtime3ds::Process3ds, at: u32, text: &str) -> (u32, u32, u32) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    process.memory.write_bytes(at, &bytes);
    (3, bytes.len() as u32, at)
}

fn open_file(host: &Host, process: &runtime3ds::Process3ds, archive: u32, text: &str, flags: u32) -> (u32, Option<u32>) {
    let (kind, size, at) = path(process, HEAP, text);
    let reply = fs::command(
        host,
        process,
        TLS,
        &request(fs::OPEN_FILE, vec![0, archive, 0, kind, size, flags, 0], vec![(size << 14) | 2, at]),
    );
    (reply.words[0], (reply.words[0] == R_OK).then(|| reply.words[2]))
}

fn file_id(host: &Host, handle: u32) -> u32 {
    match host.service_of(handle) {
        Some(Service::File(id)) => id,
        other => panic!("not a file session: {other:?}"),
    }
}

fn save_host(name: &str) -> (std::path::PathBuf, Arc<Host>) {
    let dir = staged(name);
    let host = Host::new(&dir, Vec::new());
    (dir, host)
}

#[test]
fn a_save_file_is_created_written_and_read_back() {
    let (dir, host) = save_host("fs-save");
    let process = process();
    let archive = open_archive(&host, &process, fs::ARCHIVE_SAVEDATA);
    // without the create flag a missing file is not found
    let (result, _) = open_file(&host, &process, archive, "/slot1.sav", 0b011);
    assert_eq!(result, R_PATH_NOT_FOUND);
    let (result, handle) = open_file(&host, &process, archive, "/slot1.sav", 0b111);
    assert_eq!(result, R_OK);
    let id = file_id(&host, handle.unwrap());
    process.memory.write_bytes(HEAP + 0x100, b"save!");
    let write = request(fs::FILE_WRITE, vec![0, 0, 5, 0], vec![(5 << 4) | 0xA, HEAP + 0x100]);
    assert_eq!(fs::file_command(&host, &process, TLS, id, &write).words, vec![R_OK, 5]);
    let close = request(fs::FILE_CLOSE, vec![], vec![]);
    fs::file_command(&host, &process, TLS, id, &close);
    assert_eq!(std::fs::read(dir.join("save/slot1.sav")).unwrap(), b"save!");
    // reopened read-only: the bytes and the size
    let (_, handle) = open_file(&host, &process, archive, "/slot1.sav", 0b001);
    let id = file_id(&host, handle.unwrap());
    let size = fs::file_command(&host, &process, TLS, id, &request(fs::FILE_GET_SIZE, vec![], vec![]));
    assert_eq!(size.words, vec![R_OK, 5, 0]);
    let read = request(fs::FILE_READ, vec![1, 0, 3], vec![(3 << 4) | 0xC, HEAP + 0x200]);
    assert_eq!(fs::file_command(&host, &process, TLS, id, &read).words, vec![R_OK, 3]);
    let mut out = [0u8; 3];
    process.memory.read_bytes(HEAP + 0x200, &mut out);
    assert_eq!(&out, b"ave");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn delete_and_rename_change_the_save_tree() {
    let (dir, host) = save_host("fs-rename");
    let process = process();
    std::fs::write(dir.join("save/a.bin"), b"a").unwrap();
    let archive = open_archive(&host, &process, fs::ARCHIVE_SAVEDATA);
    let (_, ss, sp) = path(&process, HEAP, "/a.bin");
    let (_, ts, tp) = path(&process, HEAP + 0x80, "/b.bin");
    let rename = request(
        fs::RENAME_FILE,
        vec![0, archive, 0, 3, ss, archive, 0, 3, ts],
        vec![(ss << 14) | 2, sp, (ts << 14) | 0x402, tp],
    );
    assert_eq!(fs::command(&host, &process, TLS, &rename).words[0], R_OK);
    assert!(dir.join("save/b.bin").exists() && !dir.join("save/a.bin").exists());
    let delete = request(fs::DELETE_FILE, vec![0, archive, 0, 3, ts], vec![(ts << 14) | 2, tp]);
    assert_eq!(fs::command(&host, &process, TLS, &delete).words[0], R_OK);
    assert!(!dir.join("save/b.bin").exists());
    // deleting it again is a not-found, never a silent success
    assert_eq!(fs::command(&host, &process, TLS, &delete).words[0], R_PATH_NOT_FOUND);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_path_cannot_climb_out_of_its_archive() {
    let (dir, host) = save_host("fs-escape");
    let process = process();
    let archive = open_archive(&host, &process, fs::ARCHIVE_SAVEDATA);
    let (result, _) = open_file(&host, &process, archive, "/../escape.bin", 0b111);
    assert_eq!(result, R_PATH_NOT_FOUND);
    assert!(!dir.join("escape.bin").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_self_ncch_romfs_reads_the_image_range_and_refuses_writes() {
    let (dir, host) = save_host("fs-romfs");
    let process = process();
    // an "image" whose RomFS level 3 starts at 0x10
    let image = dir.join("image.bin");
    let mut bytes = vec![0xEEu8; 0x10];
    bytes.extend_from_slice(b"IVFC-level-3 data");
    std::fs::write(&image, &bytes).unwrap();
    host.set_romfs_image(&image, 0x10, 17);
    // OpenFileDirectly(SelfNCCH, empty archive path, the 0xC-byte all-zero binary lowpath)
    process.memory.write_bytes(HEAP, &[0u8; 0xC]);
    let open = request(
        fs::OPEN_FILE_DIRECTLY,
        vec![0, fs::ARCHIVE_SELF_NCCH, 1, 0, 2, 0xC, 1, 0],
        vec![2, 0, (0xC << 14) | 0x402, HEAP],
    );
    let reply = fs::command(&host, &process, TLS, &open);
    assert_eq!(reply.words[0], R_OK, "{:?}", host.gaps());
    let id = file_id(&host, reply.words[2]);
    let read = request(fs::FILE_READ, vec![0, 0, 4], vec![(4 << 4) | 0xC, HEAP + 0x100]);
    assert_eq!(fs::file_command(&host, &process, TLS, id, &read).words, vec![R_OK, 4]);
    let mut out = [0u8; 4];
    process.memory.read_bytes(HEAP + 0x100, &mut out);
    assert_eq!(&out, b"IVFC");
    let write = request(fs::FILE_WRITE, vec![0, 0, 1, 0], vec![(1 << 4) | 0xA, HEAP]);
    assert_eq!(fs::file_command(&host, &process, TLS, id, &write).words[0], fs::R_WRITE_DENIED);
    // an ASCII path on SelfNCCH is not a RomFS lowpath
    let archive = open_archive(&host, &process, fs::ARCHIVE_SELF_NCCH);
    let (result, _) = open_file(&host, &process, archive, "/System/hello.bin", 1);
    assert_eq!(result, R_PATH_NOT_FOUND);
    let _ = std::fs::remove_dir_all(dir);
}
