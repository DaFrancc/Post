//! Spellchecking, as in desktop chat apps: dictionaries downloaded per
//! language on demand, several languages at once (a word is right if any of
//! them knows it), suggestions, and the user's own words. Checking happens
//! on this device; what is typed is never sent anywhere.
//!
//! Dictionaries are Chromium's Hunspell files, pinned by revision and hash
//! (see `spell_catalog`). The app downloads them; this module verifies them,
//! converts them to UTF-8 and reads them with spellbook. Offsets cross to
//! Dart in UTF-16 code units, which is how Dart indexes strings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use base64::Engine as _;
use sha2::{Digest as _, Sha256};

use super::error::{ErrorKind, PostError};
use crate::spell_catalog::{self, CATALOG};

/// A dictionary the app offers.
pub struct SpellLanguage {
    /// Its code, e.g. `en_US`.
    pub code: String,
    /// What to call it, e.g. "English (US)".
    pub name: String,
    /// Download size in bytes.
    pub bytes: u64,
    /// Downloaded and ready on this device.
    pub installed: bool,
}

/// A word no chosen dictionary knows, as a range of the checked text.
pub struct Misspelling {
    /// Start, in UTF-16 code units.
    pub start: u32,
    /// End (exclusive), in UTF-16 code units.
    pub end: u32,
}

fn files(dir: &Path, code: &str) -> (PathBuf, PathBuf) {
    (dir.join(format!("{code}.aff")), dir.join(format!("{code}.dic")))
}

fn installed(dir: &Path, code: &str) -> bool {
    let (aff, dic) = files(dir, code);
    aff.is_file() && dic.is_file()
}

fn entry(code: &str) -> Result<&'static spell_catalog::Entry, PostError> {
    spell_catalog::find(code).ok_or_else(|| PostError::new(ErrorKind::InvalidInput, "There is no such dictionary."))
}

/// Every dictionary the app offers, by name, and whether each is already
/// downloaded into `dir`.
pub fn spell_languages(dir: String) -> Vec<SpellLanguage> {
    let dir = PathBuf::from(dir);
    let mut out: Vec<SpellLanguage> = CATALOG
        .iter()
        .map(|e| SpellLanguage { code: e.code.into(), name: e.name.into(), bytes: e.bytes, installed: installed(&dir, e.code) })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Where to download a dictionary: the .aff file's URL, then the .dic's.
pub fn spell_urls(code: String) -> Result<Vec<String>, PostError> {
    let e = entry(&code)?;
    Ok(vec![spell_catalog::url(e.code, "aff"), spell_catalog::url(e.code, "dic")])
}

/// Check and install a downloaded dictionary into `dir`. `aff` and `dic` are
/// the response bodies exactly as served (base64). They must match the
/// pinned hashes; they are converted to UTF-8 before being saved.
pub async fn spell_install(dir: String, code: String, aff: Vec<u8>, dic: Vec<u8>) -> Result<(), PostError> {
    let e = entry(&code)?;
    tokio::task::spawn_blocking(move || install(Path::new(&dir), e, &aff, &dic))
        .await
        .map_err(|_| PostError::new(ErrorKind::Other, "Installing the dictionary stopped unexpectedly."))?
}

/// Delete a downloaded dictionary.
pub fn spell_remove(dir: String, code: String) -> Result<(), PostError> {
    let e = entry(&code)?;
    let (aff, dic) = files(Path::new(&dir), e.code);
    for f in [aff, dic] {
        match std::fs::remove_file(&f) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(PostError::new(ErrorKind::Storage, "The dictionary could not be removed.")),
        }
    }
    Ok(())
}

fn install(dir: &Path, e: &spell_catalog::Entry, aff: &[u8], dic: &[u8]) -> Result<(), PostError> {
    let corrupt = || PostError::new(ErrorKind::Server, format!("The {} dictionary did not download correctly. Try again.", e.name));
    let decode = |body: &[u8], want: &str| -> Result<Vec<u8>, PostError> {
        let compact: Vec<u8> = body.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
        let raw = base64::engine::general_purpose::STANDARD.decode(compact).map_err(|_| corrupt())?;
        if hex::encode(Sha256::digest(&raw)) != want {
            return Err(corrupt());
        }
        Ok(raw)
    };
    let (aff, dic) = (decode(aff, e.aff)?, decode(dic, e.dic)?);
    let (aff, dic) = to_utf8(&aff, &dic).ok_or_else(corrupt)?;
    // Make sure it reads before keeping it.
    spellbook::Dictionary::new(&aff, &dic).map_err(|_| corrupt())?;

    let storage = |_| PostError::new(ErrorKind::Storage, "The dictionary could not be saved.");
    std::fs::create_dir_all(dir).map_err(storage)?;
    let (aff_path, dic_path) = files(dir, e.code);
    // Write both, then move them into place, so a dictionary is never half there.
    let tmp = |p: &Path| p.with_extension(format!("{}.part", p.extension().and_then(|x| x.to_str()).unwrap_or("")));
    std::fs::write(tmp(&aff_path), aff).map_err(storage)?;
    std::fs::write(tmp(&dic_path), dic).map_err(storage)?;
    std::fs::rename(tmp(&dic_path), &dic_path).map_err(storage)?;
    std::fs::rename(tmp(&aff_path), &aff_path).map_err(storage)?;
    Ok(())
}

/// Hunspell files name their encoding in the .aff's `SET` line; many are
/// ISO 8859 or KOI8. Decode both files with it and declare UTF-8 instead.
fn to_utf8(aff: &[u8], dic: &[u8]) -> Option<(String, String)> {
    let label = String::from_utf8_lossy(aff)
        .lines()
        .find_map(|l| l.strip_prefix("SET").map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "UTF-8".into());
    let encoding = encoding_rs::Encoding::for_label(label.to_ascii_lowercase().as_bytes())?;
    let (aff, _, aff_bad) = encoding.decode(aff);
    let (dic, _, dic_bad) = encoding.decode(dic);
    if aff_bad || dic_bad {
        return None;
    }
    let aff = aff.lines().map(|l| if l.starts_with("SET") { "SET UTF-8" } else { l }).collect::<Vec<_>>().join("\n");
    Some((aff, dic.into_owned()))
}

/// The chosen dictionaries, loaded, and the user's own words.
pub struct Speller {
    languages: Vec<String>,
    dicts: Vec<spellbook::Dictionary>,
    own: RwLock<HashSet<String>>,
}

impl Speller {
    /// Load the dictionaries `languages` from `dir` (those not downloaded
    /// are skipped), with the user's own `words`.
    pub async fn load(dir: String, languages: Vec<String>, words: Vec<String>) -> Result<Speller, PostError> {
        tokio::task::spawn_blocking(move || {
            let dir = PathBuf::from(dir);
            let mut loaded = Vec::new();
            let mut dicts = Vec::new();
            for code in languages {
                let Ok(e) = entry(&code) else { continue };
                if !installed(&dir, e.code) {
                    continue;
                }
                let (aff, dic) = files(&dir, e.code);
                let read = |p: &Path| std::fs::read_to_string(p).ok();
                let dict = read(&aff).zip(read(&dic)).and_then(|(a, d)| spellbook::Dictionary::new(&a, &d).ok());
                if let Some(dict) = dict {
                    loaded.push(code);
                    dicts.push(dict);
                }
            }
            Speller { languages: loaded, dicts, own: RwLock::new(words.iter().map(|w| normalize(w)).collect()) }
        })
        .await
        .map_err(|_| PostError::new(ErrorKind::Other, "Loading the dictionaries stopped unexpectedly."))
    }

    /// The dictionaries actually loaded.
    pub fn languages(&self) -> Vec<String> {
        self.languages.clone()
    }

    /// The words in `text` that no chosen dictionary knows. Skips what is
    /// not prose: words with digits or underscores, all-capitals
    /// abbreviations, and anything in a chunk that looks like a link or an
    /// address.
    pub fn check(&self, text: String) -> Vec<Misspelling> {
        if self.dicts.is_empty() {
            return Vec::new();
        }
        let own = self.own.read().unwrap_or_else(|p| p.into_inner());
        words(&text)
            .into_iter()
            .filter(|w| !own.contains(&w.text) && !own.contains(&w.text.to_lowercase()) && !self.dicts.iter().any(|d| d.check(&w.text)))
            .map(|w| Misspelling { start: w.start, end: w.end })
            .collect()
    }

    /// Up to five corrections, from every chosen dictionary.
    pub fn suggest(&self, word: String) -> Vec<String> {
        let word = normalize(&word);
        let mut out: Vec<String> = Vec::new();
        for d in &self.dicts {
            let mut s = Vec::new();
            d.suggest(&word, &mut s);
            for w in s.into_iter().take(3) {
                if !out.contains(&w) {
                    out.push(w);
                }
            }
        }
        out.truncate(5);
        out
    }

    /// Accept a word from now on. The app keeps the list, and passes it to
    /// [`Speller::load`] next time.
    pub fn add_word(&self, word: String) {
        self.own.write().unwrap_or_else(|p| p.into_inner()).insert(normalize(&word));
    }

    /// Stop accepting a word added earlier.
    pub fn remove_word(&self, word: String) {
        self.own.write().unwrap_or_else(|p| p.into_inner()).remove(&normalize(&word));
    }
}

/// Typographic apostrophes as the dictionaries spell them.
fn normalize(word: &str) -> String {
    word.replace('\u{2019}', "'")
}

struct Word {
    text: String,
    start: u32,
    end: u32,
}

/// The checkable words of `text`, with UTF-16 offsets.
fn words(text: &str) -> Vec<Word> {
    let mut chars: Vec<(u32, char)> = Vec::with_capacity(text.len());
    let mut at = 0u32;
    for c in text.chars() {
        chars.push((at, c));
        at += c.len_utf16() as u32;
    }
    let end_at = at;
    let unit_at = |i: usize| chars.get(i).map_or(end_at, |c| c.0);

    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // A chunk is a run of non-space characters.
        if chars[i].1.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].1.is_whitespace() {
            i += 1;
        }
        let chunk: String = chars[start..i].iter().map(|c| c.1).collect();
        if chunk.contains("://") || chunk.starts_with("www.") || chunk.contains('@') {
            continue;
        }
        // Words: letters, digits and underscores, with apostrophes between letters.
        let mut j = start;
        while j < i {
            if !chars[j].1.is_alphanumeric() && chars[j].1 != '_' {
                j += 1;
                continue;
            }
            let ws = j;
            while j < i {
                let c = chars[j].1;
                let apostrophe = (c == '\'' || c == '\u{2019}') && j > ws && j + 1 < i && chars[j + 1].1.is_alphabetic();
                if c.is_alphanumeric() || c == '_' || apostrophe {
                    j += 1;
                } else {
                    break;
                }
            }
            let word: String = chars[ws..j].iter().map(|c| c.1).collect();
            let letters = word.chars().filter(|c| c.is_alphabetic()).count();
            let prose = letters > 1 && !word.chars().any(|c| c.is_numeric() || c == '_');
            let abbreviation = word.chars().all(|c| !c.is_lowercase());
            if prose && !abbreviation {
                out.push(Word { text: normalize(&word), start: unit_at(ws), end: unit_at(j) });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(s: &str) -> Vec<(String, u32, u32)> {
        words(s).into_iter().map(|w| (w.text, w.start, w.end)).collect()
    }

    #[test]
    fn words_skip_what_is_not_prose() {
        assert_eq!(
            texts("Don’t check https://x.io/abc or me@x.io, NASA, snake_case, v2 — but café yes"),
            vec![
                ("Don't".to_string(), 0, 5),
                ("check".into(), 6, 11),
                ("or".into(), 29, 31),
                ("but".into(), 64, 67),
                ("café".into(), 68, 72),
                ("yes".into(), 73, 76),
            ]
        );
    }

    #[test]
    fn offsets_are_utf16() {
        // The emoji is two UTF-16 units.
        assert_eq!(texts("😀 hello"), vec![("hello".to_string(), 3, 8)]);
    }

    #[test]
    fn legacy_encodings_become_utf8() {
        // "ä" in ISO 8859-1 is the single byte 0xE4.
        let (aff, dic) = to_utf8(b"SET ISO8859-1\nTRY a\n", b"1\nb\xE4r\n").expect("decodes");
        assert!(aff.starts_with("SET UTF-8"));
        assert_eq!(dic, "1\nb\u{e4}r\n");
        assert!(to_utf8(b"SET NOT-AN-ENCODING\n", b"").is_none());
    }

    #[test]
    fn only_catalogued_dictionaries_and_matching_files_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        let d = dir.path().to_string_lossy().to_string();
        assert!(spell_urls("../../etc/passwd".into()).is_err());
        let e = spell_catalog::find("en_US").expect("en_US is offered");
        let b64 = |s: &[u8]| base64::engine::general_purpose::STANDARD.encode(s).into_bytes();
        let err = install(dir.path(), e, &b64(b"SET UTF-8\n"), &b64(b"1\nhello\n")).expect_err("a mismatch is refused");
        assert_eq!(err.kind, ErrorKind::Server);
        assert!(!spell_languages(d).iter().any(|l| l.installed), "nothing was saved");
    }

    /// Checks the whole catalog against the real files: set
    /// POST_DICTIONARIES to a folder holding every catalogued .aff and
    /// .dic as downloaded (decoded from base64), and run with --ignored.
    #[tokio::test]
    #[ignore = "needs the downloaded dictionaries"]
    async fn every_catalogued_dictionary_installs_and_loads() {
        let src = PathBuf::from(std::env::var("POST_DICTIONARIES").expect("POST_DICTIONARIES"));
        let dir = tempfile::tempdir().expect("tempdir");
        let b64 = |p: PathBuf| base64::engine::general_purpose::STANDARD.encode(std::fs::read(p).expect("fixture")).into_bytes();
        for e in CATALOG {
            let (aff, dic) = files(&src, e.code);
            install(dir.path(), e, &b64(aff), &b64(dic)).unwrap_or_else(|err| panic!("{}: {}", e.code, err.message));
        }
        let d = dir.path().to_string_lossy().to_string();
        assert!(spell_languages(d.clone()).iter().all(|l| l.installed));
        let all = CATALOG.iter().map(|e| e.code.to_string()).collect();
        assert_eq!(Speller::load(d, all, vec![]).await.expect("load").languages().len(), CATALOG.len());
    }

    /// With a dictionary from this machine standing in for a download.
    #[tokio::test]
    async fn checks_against_several_languages_with_own_words() {
        let dir = tempfile::tempdir().expect("tempdir");
        let have = |c: &str| Path::new(&format!("/usr/share/hunspell/{c}.dic")).is_file();
        if !have("en_US") || !have("fr_FR") {
            return;
        }
        for c in ["en_US", "fr_FR"] {
            for ext in ["aff", "dic"] {
                std::fs::copy(format!("/usr/share/hunspell/{c}.{ext}"), dir.path().join(format!("{c}.{ext}"))).expect("copy");
            }
        }
        let d = dir.path().to_string_lossy().to_string();
        let s = Speller::load(d, vec!["en_US".into(), "fr_FR".into(), "de_DE_neu".into()], vec!["Post".into()]).await.expect("load");
        assert_eq!(s.languages(), vec!["en_US".to_string(), "fr_FR".into()], "not downloaded is skipped");
        // English, French and our own word are all right; only the typo is not.
        let found: Vec<(u32, u32)> = s.check("hello bonjour Post rihgt".into()).iter().map(|m| (m.start, m.end)).collect();
        assert_eq!(found, vec![(22, 27)]);
        assert!(s.suggest("rihgt".into()).iter().any(|w| w == "right"));
        s.add_word("rihgt".into());
        assert!(s.check("rihgt".into()).is_empty());
        s.remove_word("rihgt".into());
        assert_eq!(s.check("rihgt".into()).len(), 1);
    }
}
