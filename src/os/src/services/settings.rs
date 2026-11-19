//! nn::settings: the language codes the system offers (the fixed table, in nn::settings::Language order) and the
//! comparisons; the desired language is oe's (the `language` setting).

use crate::guest::Args;
use crate::hle::Registry;
use nxrt::cpu::Cpu;

pub const LANG_CODES: [&str; 18] = ["ja", "en-US", "fr", "de", "it", "es", "zh-CN", "ko", "nl", "pt", "ru", "zh-TW", "en-GB", "fr-CA", "es-419", "zh-Hans", "zh-Hant", "pt-BR"];

pub fn code(lang: i32) -> u64 {
    let mut b = [0u8; 8];
    if let Some(s) = LANG_CODES.get(lang as usize) {
        b[..s.len()].copy_from_slice(s.as_bytes());
    }
    u64::from_le_bytes(b)
}

fn matches(lang: i32, code_at: u64) -> bool {
    let c = crate::guest::cstr(code_at, 8);
    let want = LANG_CODES.get(lang as usize).copied().unwrap_or("");
    c == want || (lang == 6 && c == "zh-Hans") || (lang == 11 && c == "zh-Hant") || (lang == 15 && c == "zh-CN") || (lang == 16 && c == "zh-TW")
}

unsafe fn make(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let v = code(a.i(0));
    a.ret(v);
}
unsafe fn available(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let (out, n) = (a.x(0), a.i(1).max(0) as usize);
    let k = n.min(LANG_CODES.len());
    for i in 0..k {
        crate::guest::w64(out + 8 * i as u64, code(i as i32));
    }
    a.ret(k as u64);
}
unsafe fn available_count(c: *mut Cpu) {
    (*c).x[0] = LANG_CODES.len() as u64;
}
unsafe fn eq_lang_code(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let r = matches(crate::guest::r32(a.x(0)) as i32, a.x(1));
    a.ret(r as u64);
}
unsafe fn eq_code_lang(c: *mut Cpu) {
    let mut a = Args(&mut *c);
    let r = matches(crate::guest::r32(a.x(1)) as i32, a.x(0));
    a.ret(r as u64);
}

pub fn register(r: &mut Registry) {
    const W: &str = "set: the fixed language table";
    r.force("_ZN2nn8settings12LanguageCode4MakeENS0_8LanguageE", W, make);
    r.force("_ZN2nn8settings25GetAvailableLanguageCodesEPNS0_12LanguageCodeEi", W, available);
    r.force("_ZN2nn8settings29GetAvailableLanguageCodeCountEv", W, available_count);
    r.force("_ZN2nn8settingseqERKNS0_8LanguageERKNS0_12LanguageCodeE", W, eq_lang_code);
    r.force("_ZN2nn8settingseqERKNS0_12LanguageCodeERKNS0_8LanguageE", W, eq_code_lang);
}

#[cfg(test)]
mod tests {
    #[test]
    fn codes() {
        assert_eq!(super::code(1).to_le_bytes()[..5], *b"en-US");
        assert_eq!(super::code(99), 0);
    }
}
