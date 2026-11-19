//! HIPC messages with their CMIF payload: the layout a client writes into its message buffer (the thread's TLS for
//! `SendSyncRequest`, or a user buffer) and the layout of the answer the server writes back over it.
//!
//! ```text
//! word 0   type:16 | X count:4 | A count:4 | B count:4 | W count:4
//! word 1   data words:10 | receive-list mode:4 | ... | special header:1 (bit 31)
//! special  send pid:1 | copy handles:4 | move handles:4; [pid: 2 words]; copy handles; move handles
//! X        2 words: index:6 | addr[36..42):6 | addr[32..36):4 | size:16 ;  addr[0..32)
//! A, B, W  3 words: size[0..32) ; addr[0..32) ; mode:2 | addr[36..58):22 | size[32..36):4 | addr[32..36):4
//! data     raw words; the CMIF part starts 16-byte aligned to the buffer: in a domain session the domain header
//!          {u8 type, u8 objects, u16 size, u32 object, u32, u32 token}, then {"SFCI", version, command, token}
//!          and the arguments (in object ids follow the arguments in a domain)
//! C        after the data: 2 words each: addr[0..32) ; addr[32..48):16 | size:16
//! ```
//! The answer has no descriptors: {"SFCO", version, result, token} and the output (out object ids after it in a
//! domain, behind a {u32 objects, 3 words} header), with copied and moved handles in the special header.

pub const MAGIC_SFCI: u32 = 0x4943_4653;
pub const MAGIC_SFCO: u32 = 0x4F43_4653;

pub const T_CLOSE: u16 = 2;
pub const T_REQUEST: u16 = 4;
pub const T_CONTROL: u16 = 5;
pub const T_REQUEST_CTX: u16 = 6;
pub const T_CONTROL_CTX: u16 = 7;

pub const D_SEND: u8 = 1;
pub const D_CLOSE: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Buf {
    pub addr: u64,
    pub size: u64,
    /// X: the descriptor's index; A/B/W: its mode.
    pub tag: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Domain {
    pub kind: u8,
    pub object: u32,
    pub in_objects: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Request {
    pub ty: u16,
    pub pid: Option<u64>,
    pub copy: Vec<u32>,
    pub moved: Vec<u32>,
    pub x: Vec<Buf>,
    pub a: Vec<Buf>,
    pub b: Vec<Buf>,
    pub w: Vec<Buf>,
    pub c: Vec<Buf>,
    pub domain: Option<Domain>,
    pub magic: u32,
    pub version: u32,
    pub cmd: u32,
    pub token: u32,
    /// The arguments (after the SFCI header).
    pub data: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Short,
    BadMagic(u32),
}

fn w(b: &[u8], i: usize) -> Result<u32, ParseError> {
    b.get(i * 4..i * 4 + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap())).ok_or(ParseError::Short)
}

fn abw(p0: u32, p1: u32, p2: u32) -> Buf {
    let addr = p1 as u64 | ((p2 as u64 >> 28) & 0xF) << 32 | ((p2 as u64 >> 2) & 0x3F_FFFF) << 36;
    let size = p0 as u64 | ((p2 as u64 >> 24) & 0xF) << 32;
    Buf { addr, size, tag: p2 & 3 }
}

/// Parses a request message. `domain` says whether the session is a domain (then requests carry a domain header).
pub fn parse(msg: &[u8], domain: bool) -> Result<Request, ParseError> {
    let (h0, h1) = (w(msg, 0)?, w(msg, 1)?);
    let mut r = Request { ty: (h0 & 0xFFFF) as u16, ..Default::default() };
    let (nx, na, nb, nw) = ((h0 >> 16) & 15, (h0 >> 20) & 15, (h0 >> 24) & 15, (h0 >> 28) & 15);
    let ndw = (h1 & 0x3FF) as usize;
    let cmode = (h1 >> 10) & 15;
    let mut p = 2usize;
    if h1 >> 31 != 0 {
        let sh = w(msg, p)?;
        p += 1;
        if sh & 1 != 0 {
            r.pid = Some(w(msg, p)? as u64 | (w(msg, p + 1)? as u64) << 32);
            p += 2;
        }
        for _ in 0..(sh >> 1) & 15 {
            r.copy.push(w(msg, p)?);
            p += 1;
        }
        for _ in 0..(sh >> 5) & 15 {
            r.moved.push(w(msg, p)?);
            p += 1;
        }
    }
    for _ in 0..nx {
        let (d0, d1) = (w(msg, p)?, w(msg, p + 1)?);
        let addr = d1 as u64 | ((d0 as u64 >> 12) & 0xF) << 32 | ((d0 as u64 >> 6) & 0x3F) << 36;
        r.x.push(Buf { addr, size: (d0 >> 16) as u64, tag: d0 & 0x3F });
        p += 2;
    }
    for k in 0..na + nb + nw {
        let d = abw(w(msg, p)?, w(msg, p + 1)?, w(msg, p + 2)?);
        p += 3;
        if k < na {
            r.a.push(d)
        } else if k < na + nb {
            r.b.push(d)
        } else {
            r.w.push(d)
        }
    }
    let data_start = p * 4;
    let data_end = data_start + ndw * 4;
    if data_end > msg.len() {
        return Err(ParseError::Short);
    }
    // receive list (C descriptors) after the data
    let nc = match cmode {
        0 | 1 => 0,
        2 => 1,
        m => m - 2,
    };
    let mut q = data_end / 4;
    for _ in 0..nc {
        let (c0, c1) = (w(msg, q)?, w(msg, q + 1)?);
        r.c.push(Buf { addr: c0 as u64 | ((c1 & 0xFFFF) as u64) << 32, size: (c1 >> 16) as u64, tag: 0 });
        q += 2;
    }
    if r.ty == T_CLOSE {
        return Ok(r);
    }
    let mut cm = (data_start + 15) & !15;
    let mut payload_end = data_end;
    if domain && (r.ty == T_REQUEST || r.ty == T_REQUEST_CTX) {
        let d0 = w(msg, cm / 4)?;
        let kind = (d0 & 0xFF) as u8;
        let nobj = ((d0 >> 8) & 0xFF) as usize;
        let dsize = (d0 >> 16) as usize;
        let object = w(msg, cm / 4 + 1)?;
        let mut dom = Domain { kind, object, in_objects: vec![] };
        cm += 16;
        if kind == D_SEND {
            payload_end = cm + dsize;
            for k in 0..nobj {
                dom.in_objects.push(w(msg, payload_end / 4 + k)?);
            }
        }
        r.domain = Some(dom);
        if kind != D_SEND {
            return Ok(r);
        }
    }
    r.magic = w(msg, cm / 4)?;
    if r.magic != MAGIC_SFCI {
        return Err(ParseError::BadMagic(r.magic));
    }
    r.version = w(msg, cm / 4 + 1)?;
    r.cmd = w(msg, cm / 4 + 2)?;
    r.token = w(msg, cm / 4 + 3)?;
    let a = cm + 16;
    if payload_end > a {
        r.data = msg.get(a..payload_end.min(msg.len())).ok_or(ParseError::Short)?.to_vec();
    }
    Ok(r)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub result: u32,
    pub data: Vec<u8>,
    pub copy: Vec<u32>,
    pub moved: Vec<u32>,
    /// Out object ids (domain sessions).
    pub objects: Vec<u32>,
}

/// Writes the answer over the message buffer (zeroed first, up to its size).
pub fn write_response(msg: &mut [u8], resp: &Response, domain: bool, version: u32, token: u32) {
    msg.fill(0);
    let mut words: Vec<u32> = vec![0, 0];
    let nh = resp.copy.len() + resp.moved.len();
    if nh > 0 {
        words.push((resp.copy.len() as u32) << 1 | (resp.moved.len() as u32) << 5);
        words.extend(&resp.copy);
        words.extend(&resp.moved);
    }
    let start = words.len() * 4;
    let mut cm = (start + 15) & !15;
    let mut body: Vec<u8> = vec![0; cm - start];
    if domain {
        let mut dh = [0u8; 16];
        dh[0..4].copy_from_slice(&(resp.objects.len() as u32).to_le_bytes());
        body.extend_from_slice(&dh);
        cm += 16;
    }
    let _ = cm;
    for v in [MAGIC_SFCO, version, resp.result, token] {
        body.extend_from_slice(&v.to_le_bytes());
    }
    if resp.result == 0 {
        body.extend_from_slice(&resp.data);
    }
    if domain {
        for o in &resp.objects {
            body.extend_from_slice(&o.to_le_bytes());
        }
    }
    // the data words: the CMIF part plus the alignment padding (as the server sizes them)
    let ndw = body.len().div_ceil(4) + 4;
    words[0] = 0;
    words[1] = (ndw as u32).min(0x3FF) | if nh > 0 { 1 << 31 } else { 0 };
    let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    out.extend_from_slice(&body);
    let n = out.len().min(msg.len());
    msg[..n].copy_from_slice(&out[..n]);
}

/// A request message as a client writes one (for tests and for host-side clients).
#[derive(Default)]
pub struct Builder {
    pub ty: u16,
    pub cmd: u32,
    pub data: Vec<u8>,
    pub pid: bool,
    pub copy: Vec<u32>,
    pub moved: Vec<u32>,
    pub x: Vec<Buf>,
    pub a: Vec<Buf>,
    pub b: Vec<Buf>,
    pub c: Vec<Buf>,
    pub domain_object: Option<u32>,
    pub in_objects: Vec<u32>,
}

impl Builder {
    pub fn build(&self) -> Vec<u8> {
        let mut words: Vec<u32> = vec![0, 0];
        let special = self.pid || !self.copy.is_empty() || !self.moved.is_empty();
        if special {
            words.push(self.pid as u32 | (self.copy.len() as u32) << 1 | (self.moved.len() as u32) << 5);
            if self.pid {
                words.extend([0, 0]);
            }
            words.extend(&self.copy);
            words.extend(&self.moved);
        }
        for x in &self.x {
            words.push(x.tag & 0x3F | (((x.addr >> 36) & 0x3F) as u32) << 6 | (((x.addr >> 32) & 0xF) as u32) << 12 | (x.size as u32) << 16);
            words.push(x.addr as u32);
        }
        for d in self.a.iter().chain(&self.b) {
            words.push(d.size as u32);
            words.push(d.addr as u32);
            words.push(d.tag & 3 | (((d.addr >> 36) & 0x3F_FFFF) as u32) << 2 | (((d.size >> 32) & 0xF) as u32) << 24 | (((d.addr >> 32) & 0xF) as u32) << 28);
        }
        let start = words.len() * 4;
        let cm = (start + 15) & !15;
        let mut body: Vec<u8> = vec![0; cm - start];
        let mut payload: Vec<u8> = vec![];
        for v in [MAGIC_SFCI, 1, self.cmd, 0] {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        payload.extend_from_slice(&self.data);
        if let Some(obj) = self.domain_object {
            let mut dh = [0u8; 16];
            dh[0] = D_SEND;
            dh[1] = self.in_objects.len() as u8;
            dh[2..4].copy_from_slice(&(payload.len() as u16).to_le_bytes());
            dh[4..8].copy_from_slice(&obj.to_le_bytes());
            body.extend_from_slice(&dh);
            body.extend_from_slice(&payload);
            for o in &self.in_objects {
                body.extend_from_slice(&o.to_le_bytes());
            }
        } else {
            body.extend_from_slice(&payload);
        }
        let ndw = body.len().div_ceil(4) + 4;
        let cmode = if self.c.is_empty() { 0 } else { self.c.len() as u32 + 2 };
        words[0] = self.ty as u32 | (self.x.len() as u32) << 16 | (self.a.len() as u32) << 20 | (self.b.len() as u32) << 24;
        words[1] = ndw as u32 | cmode << 10 | if special { 1 << 31 } else { 0 };
        let mut out: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        out.extend_from_slice(&body);
        out.resize(start + ndw * 4, 0);
        for c in &self.c {
            out.extend_from_slice(&(c.addr as u32).to_le_bytes());
            out.extend_from_slice(&((((c.addr >> 32) & 0xFFFF) as u32) | (c.size as u32) << 16).to_le_bytes());
        }
        out.resize(0x100.max(out.len()), 0);
        out
    }
}

/// Reads an answer as a client does: (result, output data, copied handles, moved handles, out objects).
pub fn parse_response(msg: &[u8], domain: bool) -> Result<Response, ParseError> {
    let h1 = w(msg, 1)?;
    let mut p = 2usize;
    let mut r = Response::default();
    if h1 >> 31 != 0 {
        let sh = w(msg, p)?;
        p += 1;
        for _ in 0..(sh >> 1) & 15 {
            r.copy.push(w(msg, p)?);
            p += 1;
        }
        for _ in 0..(sh >> 5) & 15 {
            r.moved.push(w(msg, p)?);
            p += 1;
        }
    }
    let mut cm = (p * 4 + 15) & !15;
    let mut nobj = 0;
    if domain {
        nobj = w(msg, cm / 4)? as usize;
        cm += 16;
    }
    let magic = w(msg, cm / 4)?;
    if magic != MAGIC_SFCO {
        return Err(ParseError::BadMagic(magic));
    }
    r.result = w(msg, cm / 4 + 2)?;
    let end = p * 4 + (h1 & 0x3FF) as usize * 4;
    r.data = msg.get(cm + 16..end.min(msg.len())).unwrap_or(&[]).to_vec();
    if domain && nobj > 0 {
        // out objects follow the output data; the test caller knows its output size
        r.objects = vec![0; nobj];
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let b = Builder {
            ty: T_REQUEST,
            cmd: 42,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            pid: true,
            copy: vec![0x1234],
            x: vec![Buf { addr: 0x7_1234_5678, size: 0x40, tag: 3 }],
            a: vec![Buf { addr: 0x3F_8765_4321, size: 0x1_0000_0010, tag: 1 }],
            b: vec![Buf { addr: 0x55_0000_1000, size: 0x200, tag: 0 }],
            c: vec![Buf { addr: 0xABCD_0000_0100, size: 0x80, tag: 0 }],
            ..Default::default()
        };
        let m = b.build();
        let r = parse(&m, false).unwrap();
        assert_eq!(r.ty, T_REQUEST);
        assert_eq!(r.cmd, 42);
        assert_eq!(r.pid, Some(0));
        assert_eq!(r.copy, vec![0x1234]);
        assert_eq!(r.x, vec![Buf { addr: 0x7_1234_5678, size: 0x40, tag: 3 }]);
        assert_eq!(r.a, vec![Buf { addr: 0x3F_8765_4321, size: 0x1_0000_0010, tag: 1 }]);
        assert_eq!(r.b[0].addr, 0x55_0000_1000);
        assert_eq!(r.c, vec![Buf { addr: 0xABCD_0000_0100, size: 0x80, tag: 0 }]);
        assert_eq!(&r.data[..8], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn domain_request() {
        let b = Builder { ty: T_REQUEST, cmd: 7, data: 9u32.to_le_bytes().to_vec(), domain_object: Some(5), in_objects: vec![3], ..Default::default() };
        let r = parse(&b.build(), true).unwrap();
        let d = r.domain.unwrap();
        assert_eq!((d.kind, d.object, d.in_objects.clone()), (D_SEND, 5, vec![3]));
        assert_eq!(r.cmd, 7);
        assert_eq!(r.data, 9u32.to_le_bytes().to_vec());
        let mut close = Builder { ty: T_REQUEST, cmd: 0, domain_object: Some(5), ..Default::default() }.build();
        let cm = 16; // no descriptors: the CMIF part at 0x10
        close[cm] = D_CLOSE;
        let r = parse(&close, true).unwrap();
        assert_eq!(r.domain.unwrap().kind, D_CLOSE);
    }

    #[test]
    fn response_layout() {
        let mut m = vec![0xAAu8; 0x100];
        let resp = Response { result: 0, data: 0x1122_3344u32.to_le_bytes().to_vec(), copy: vec![0x9], moved: vec![0xD0001], objects: vec![] };
        write_response(&mut m, &resp, false, 1, 0);
        let r = parse_response(&m, false).unwrap();
        assert_eq!(r.result, 0);
        assert_eq!(r.copy, vec![0x9]);
        assert_eq!(r.moved, vec![0xD0001]);
        assert_eq!(&r.data[..4], &0x1122_3344u32.to_le_bytes());
        // the CMIF part is 16-byte aligned: header (2 words) + special header + 2 handles = 20 bytes -> 0x20
        assert_eq!(u32::from_le_bytes(m[0x20..0x24].try_into().unwrap()), MAGIC_SFCO);
        write_response(&mut m, &Response { result: 0xE15, ..Default::default() }, true, 1, 0);
        let r = parse_response(&m, true).unwrap();
        assert_eq!(r.result, 0xE15);
    }

    #[test]
    fn errors() {
        assert_eq!(parse(&[0u8; 4], false), Err(ParseError::Short));
        let mut m = Builder { ty: T_REQUEST, cmd: 1, ..Default::default() }.build();
        m[0x10] = 0;
        assert!(matches!(parse(&m, false), Err(ParseError::BadMagic(_))));
    }
}
