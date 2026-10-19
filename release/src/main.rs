//! Signs and publishes mint releases; see `release.sh` and `server/README.md`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use ed25519_dalek::SigningKey;
use remote_preset::release::{LINUX, Manifest, PUBLIC_KEY, ReleaseFile, SignedManifest, WINDOWS};
use sha2::{Digest, Sha256};

const USAGE: &str = "\
usage:
  mint_release keygen                 create the signing key; prints the public key for
                                      remote_preset/src/release.rs
  mint_release next                   print the next release number
  mint_release publish <n> <linux binary> <windows exe> [notes]
                                      upload and publish release <n>
  mint_release download <dir>         download and verify the latest release into <dir>
                                      (mint, mint.exe); prints its number

environment:
  MINT_RELEASE_KEY   signing key file (default ~/.local/share/mint-release/signing.key)
  MINT_SERVER        server (default: mint's preset server setting)
  MINT_PUBLISH_KEY   publish key (default: mint's publish key setting)";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["keygen"] => keygen(),
        ["next"] => {
            println!("{}", latest(&server()?)?.map_or(0, |m| m.release) + 1);
            Ok(())
        }
        ["publish", n, linux, windows, notes @ ..] if notes.len() <= 1 => publish(
            n.parse().context("release number")?,
            Path::new(linux),
            Path::new(windows),
            notes.first().copied().unwrap_or_default(),
        ),
        ["download", dir] => download(Path::new(dir)),
        _ => bail!("{USAGE}"),
    }
}

fn home() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME").context("HOME is not set")?,
    ))
}

fn key_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("MINT_RELEASE_KEY") {
        return Ok(p.into());
    }
    let data = match std::env::var_os("XDG_DATA_HOME") {
        Some(d) => PathBuf::from(d),
        None => home()?.join(".local/share"),
    };
    Ok(data.join("mint-release/signing.key"))
}

/// mint's config.json, for the server URL and publish key.
fn mint_config() -> Result<serde_json::Value> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(d) => PathBuf::from(d),
        None => home()?.join(".config"),
    };
    let path = config.join("mint/config.json");
    let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn server() -> Result<String> {
    if let Ok(s) = std::env::var("MINT_SERVER") {
        return Ok(s.trim_end_matches('/').to_string());
    }
    let configured = mint_config()
        .ok()
        .and_then(|c| c["preset_server"].as_str().map(str::to_string));
    Ok(configured.unwrap_or_else(|| "https://mint.kohlroulade.lgbt".into()))
}

fn publish_key() -> Result<String> {
    if let Ok(k) = std::env::var("MINT_PUBLISH_KEY") {
        return Ok(k);
    }
    mint_config()?["publish_key"]
        .as_str()
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .context("no publish key: set MINT_PUBLISH_KEY or mint's publish key setting")
}

fn client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("mint_release/", env!("CARGO_PKG_VERSION")))
        // uploads of ~50 MB over a home uplink
        .timeout(Duration::from_secs(1800))
        .build()?)
}

/// Fails with the server's `{"error": …}` message.
fn check(res: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
    if res.status().is_success() {
        return Ok(res);
    }
    let status = res.status();
    let body = res.text().unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"].as_str().map(str::to_string))
        .unwrap_or(body);
    bail!("{status}: {msg}")
}

fn keygen() -> Result<()> {
    let path = key_path()?;
    ensure!(
        !path.exists(),
        "{} exists already; a new key would make every installed mint refuse updates",
        path.display()
    );
    fs::create_dir_all(path.parent().unwrap())?;
    fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o700))?;
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    writeln!(f, "{}", hex::encode(seed))?;
    f.sync_all()?;
    let public = hex::encode(SigningKey::from_bytes(&seed).verifying_key().to_bytes());
    println!(
        "signing key: {} (back it up; it can't be recovered)",
        path.display()
    );
    println!("public key:  {public}");
    Ok(())
}

fn signing_key() -> Result<SigningKey> {
    let path = key_path()?;
    let text = fs::read_to_string(&path)
        .with_context(|| format!("reading {} (create it with `keygen`)", path.display()))?;
    let seed: [u8; 32] = hex::decode(text.trim())
        .ok()
        .and_then(|s| s.try_into().ok())
        .with_context(|| format!("{} is no signing key", path.display()))?;
    let key = SigningKey::from_bytes(&seed);
    ensure!(
        hex::encode(key.verifying_key().to_bytes()) == PUBLIC_KEY,
        "{} doesn't match the PUBLIC_KEY compiled into mint; installed mints would refuse this release",
        path.display()
    );
    Ok(key)
}

/// The latest release, verified. `None` if the server has none yet.
fn latest(server: &str) -> Result<Option<Manifest>> {
    let res = client()?
        .get(format!("{server}/api/v1/releases/latest"))
        .send()?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let signed: SignedManifest = check(res)?.json()?;
    Ok(Some(signed.verify(PUBLIC_KEY).map_err(anyhow::Error::msg)?))
}

fn sha256_file(path: &Path) -> Result<(u64, String)> {
    let mut f = fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    let mut size = 0;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((size, hex::encode(hasher.finalize())))
}

fn publish(release: u32, linux: &Path, windows: &Path, notes: &str) -> Result<()> {
    let key = signing_key()?;
    let server = server()?;
    let token = publish_key()?;
    let next = latest(&server)?.map_or(0, |m| m.release) + 1;
    ensure!(
        release == next,
        "the server's next release is {next}, not {release}; rebuild with MINT_RELEASE={next}"
    );

    let mut files = BTreeMap::new();
    for (target, path, name) in [
        (LINUX, linux, format!("mint-{release}")),
        (WINDOWS, windows, format!("mint-{release}.exe")),
    ] {
        let (size, sha256) = sha256_file(path)?;
        eprintln!(
            "uploading {} as {name} ({:.1} MB)",
            path.display(),
            size as f64 / 1e6
        );
        check(
            client()?
                .put(format!("{server}/api/v1/releases/{release}/files/{name}"))
                .bearer_auth(&token)
                .body(fs::File::open(path)?)
                .send()?,
        )?;
        files.insert(target.to_string(), ReleaseFile { name, size, sha256 });
    }

    let manifest = Manifest {
        release,
        published: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        notes: notes.to_string(),
        files,
    };
    let signed = SignedManifest::sign(&manifest, &key);
    signed.verify(PUBLIC_KEY).map_err(anyhow::Error::msg)?;
    check(
        client()?
            .put(format!("{server}/api/v1/releases/{release}"))
            .bearer_auth(&token)
            .json(&signed)
            .send()?,
    )?;
    println!("published release {release} to {server}");
    Ok(())
}

fn download(dir: &Path) -> Result<()> {
    let server = server()?;
    let manifest = latest(&server)?.context("the server has no release yet")?;
    fs::create_dir_all(dir)?;
    for (target, local, mode) in [(LINUX, "mint", 0o755), (WINDOWS, "mint.exe", 0o644)] {
        let file = manifest
            .files
            .get(target)
            .with_context(|| format!("release {} has no {target} build", manifest.release))?;
        let url = format!(
            "{server}/api/v1/releases/{}/files/{}",
            manifest.release, file.name
        );
        let bytes = check(client()?.get(&url).send()?)?.bytes()?;
        ensure!(
            bytes.len() as u64 == file.size && hex::encode(Sha256::digest(&bytes)) == file.sha256,
            "{url} doesn't match the signed manifest"
        );
        let path = dir.join(local);
        fs::write(&path, &bytes)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
    }
    println!("{}", manifest.release);
    Ok(())
}
