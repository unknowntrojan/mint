//! Publish keys. The keys file holds one `<publisher> <sha256 of key, hex>` per line (`#` starts a
//! comment), so the file itself can't be used to publish.

use std::io;
use std::path::Path;

use sha2::{Digest, Sha256};

/// A new random key and the line to add to the keys file for it.
pub fn generate(publisher: &str) -> Result<(String, String), String> {
    if publisher.is_empty()
        || !publisher
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("publisher name may only contain letters, digits, '-' and '_'".into());
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| e.to_string())?;
    let key = hex::encode(raw);
    let line = format!(
        "{publisher} {}",
        hex::encode(Sha256::digest(key.as_bytes()))
    );
    Ok((key, line))
}

/// Name of the publisher owning `key`, if any. A missing keys file means nobody can publish.
pub fn check(keys_file: &Path, key: &str) -> io::Result<Option<String>> {
    let contents = match std::fs::read_to_string(keys_file) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let hash = Sha256::digest(key.as_bytes());
    let mut found = None;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap().trim();
        let Some((publisher, stored)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(stored) = hex::decode(stored.trim()) else {
            continue;
        };
        // compare every entry without exiting early
        if stored.len() == hash.len()
            && stored
                .iter()
                .zip(hash.iter())
                .fold(0, |acc, (a, b)| acc | (a ^ b))
                == 0
        {
            found = Some(publisher.to_string());
        }
    }
    Ok(found)
}
