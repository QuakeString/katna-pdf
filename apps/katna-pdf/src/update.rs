// SPDX-License-Identifier: GPL-3.0-or-later

//! Updates from inside the app (docs/PLAN.md §8, Phase 5): Katna PDF looks
//! for a newer build of its own kind of package on the GitHub release CI
//! publishes it on, and, where it can, downloads it, checks it and installs
//! it, then starts again. No GPUI here.
//!
//! Each release carries `katna-pdf-update.txt` (made by
//! `packaging/update-manifest.sh`): the build's version, then each file's
//! SHA-256, size and name. A download is kept only if it has exactly that
//! size and hash.
//!
//! - The Arch package installs through `pkexec` and
//!   `/usr/lib/katna-pdf/katna-pdf-update-helper`, so the system asks for
//!   the password (`packaging/polkit/`). The helper checks the file again
//!   as root, that it is `katna-pdf-git` and that it is newer.
//! - An AppImage replaces itself, beside where it is.
//! - On Windows the new Katna PDF Setup updates the install quietly (asking
//!   for an administrator for an install for everyone), then opens Katna
//!   PDF again.
//! - The .deb, the .rpm, the tarball and the Windows zip only say that a
//!   newer build is out, with a link to it: they are installed by the
//!   system's tools or by hand.
//!
//! Builds that do not know their version (`cargo build` without
//! `KATNA_PDF_VERSION`) never look. `KATNA_PDF_UPDATES=0` turns looking off;
//! `KATNA_PDF_UPDATE_URL` points it at another server, for tests.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// This build's version, as the packages name it (`packaging/linux/version.sh`):
/// set by CI and by the Arch package, absent in a plain `cargo build`.
pub const VERSION: Option<&str> = option_env!("KATNA_PDF_VERSION");

/// Where the builds are published.
const RELEASES: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

/// The file beside each build that describes it.
pub const MANIFEST: &str = "katna-pdf-update.txt";

/// How often a running Katna PDF looks again.
pub const EVERY: Duration = Duration::from_secs(12 * 60 * 60);

/// The longest manifest read.
const MAX_MANIFEST: u64 = 64 * 1024;

/// The largest download taken.
const MAX_SIZE: u64 = 512 * 1024 * 1024;

/// The Arch package's name.
const ARCH_PACKAGE: &str = "katna-pdf-git";

/// The helper that installs the Arch package as root (`packaging/arch`).
pub const ARCH_HELPER: &str = "/usr/lib/katna-pdf/katna-pdf-update-helper";

/// Where the Arch package, the .deb and the .rpm put the app.
const SYSTEM_EXE: &str = "/usr/bin/katna-pdf";

/// How this Katna PDF was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Package {
    /// The Arch package, `katna-pdf-git`.
    Arch,
    /// The AppImage at this path.
    AppImage(PathBuf),
    /// The .deb or the .rpm.
    System,
    /// The tarball, or a build run from somewhere else.
    Tarball,
    /// Katna PDF Setup's install; `everyone` for one in Program Files.
    Windows { everyone: bool },
    /// The Windows zip, unpacked by hand.
    WindowsZip,
}

impl Package {
    /// The package this Katna PDF runs from.
    pub fn current() -> Self {
        let exe = std::env::current_exe().unwrap_or_default();
        if cfg!(windows) {
            let setup = exe.with_file_name("katna-pdf-setup.exe");
            if !setup.is_file() {
                return Self::WindowsZip;
            }
            let everyone =
                std::env::var_os("ProgramFiles").is_some_and(|programs| exe.starts_with(programs));
            return Self::Windows { everyone };
        }
        if let Some(image) = std::env::var_os("APPIMAGE").map(PathBuf::from)
            && image.is_file()
        {
            return Self::AppImage(image);
        }
        if exe != Path::new(SYSTEM_EXE) {
            return Self::Tarball;
        }
        let arch = fs::read_dir("/var/lib/pacman/local").is_ok_and(|mut dir| {
            dir.any(|e| {
                e.is_ok_and(|e| {
                    let name = e.file_name();
                    let name = name.to_string_lossy();
                    name.strip_prefix(ARCH_PACKAGE)
                        .is_some_and(|rest| rest.starts_with('-'))
                })
            })
        });
        if arch { Self::Arch } else { Self::System }
    }

    /// The release its builds are published on.
    pub fn release(&self) -> &'static str {
        match self {
            Self::Arch => "arch-latest",
            Self::AppImage(_) | Self::System | Self::Tarball => "linux-latest",
            Self::Windows { .. } | Self::WindowsZip => "windows-latest",
        }
    }

    /// The file it updates itself from; `None` when it is updated by
    /// other means.
    pub fn file(&self) -> Option<&'static str> {
        match self {
            Self::Arch => Some("katna-pdf-git-x86_64.pkg.tar.zst"),
            Self::AppImage(_) => Some("Katna-PDF-x86_64.AppImage"),
            Self::Windows { .. } => Some("KatnaPDFSetup-x86_64.exe"),
            Self::System | Self::Tarball | Self::WindowsZip => None,
        }
    }

    /// The release's page, for builds that are not updated from here.
    pub fn page(&self) -> String {
        format!("{RELEASES}/tag/{}", self.release())
    }
}

/// One file of a build, as the manifest describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub sha256: String,
    pub size: u64,
}

/// A build on a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub files: Vec<Entry>,
}

impl Manifest {
    /// Reads a manifest: `version V`, then `file SHA256 SIZE NAME` lines.
    /// Anything else, or a version or hash that is not one, refuses it.
    pub fn parse(text: &str) -> Option<Self> {
        let mut version = None;
        let mut files = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let mut words = line.split_whitespace();
            match words.next()? {
                "version" => {
                    let v = words.next()?;
                    parse_version(v)?;
                    version = Some(v.to_owned());
                }
                "file" => {
                    let sha256 = words.next()?;
                    let size = words.next()?.parse().ok()?;
                    let name = words.next()?;
                    let hex = sha256.len() == 64
                        && sha256
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                    let plain = name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-+".contains(&b));
                    if !hex || !plain || name.starts_with('.') || size > MAX_SIZE {
                        return None;
                    }
                    files.push(Entry {
                        name: name.to_owned(),
                        sha256: sha256.to_owned(),
                        size,
                    });
                }
                _ => return None,
            }
            if words.next().is_some() {
                return None;
            }
        }
        Some(Self {
            version: version?,
            files,
        })
    }

    pub fn file(&self, name: &str) -> Option<&Entry> {
        self.files.iter().find(|f| f.name == name)
    }
}

/// A version's parts that order it: `X.Y.Z` and the commits since its
/// tag (`X.Y.Z.rN.gHASH`), 0 for a plain `X.Y.Z`.
fn parse_version(version: &str) -> Option<[u64; 4]> {
    let mut parts = version.split('.');
    let mut out = [0; 4];
    for slot in &mut out[..3] {
        *slot = parts.next()?.parse().ok()?;
    }
    match (parts.next(), parts.next(), parts.next()) {
        (None, ..) => {}
        (Some(r), Some(g), None) => {
            out[3] = r.strip_prefix('r')?.parse().ok()?;
            let hash = g.strip_prefix('g')?;
            if hash.is_empty() || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
        }
        _ => return None,
    }
    Some(out)
}

/// Whether `offered` is a later build than `installed`.
pub fn newer(installed: &str, offered: &str) -> bool {
    match (parse_version(installed), parse_version(offered)) {
        (Some(a), Some(b)) => b > a,
        _ => false,
    }
}

/// What a check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    /// The file to install, for packages that install themselves.
    pub file: Option<Entry>,
    pub package: Package,
}

/// Whether this build looks for updates at all.
pub fn enabled() -> bool {
    VERSION.is_some() && std::env::var_os("KATNA_PDF_UPDATES").is_none_or(|v| v != "0")
}

fn base() -> String {
    std::env::var("KATNA_PDF_UPDATE_URL").unwrap_or_else(|_| format!("{RELEASES}/download"))
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

/// Looks for a build newer than this one. `Ok(None)` when there is none.
/// Blocks; run it away from the window.
pub fn check() -> Result<Option<Offer>, String> {
    let Some(installed) = VERSION else {
        return Ok(None);
    };
    let package = Package::current();
    let url = format!("{}/{}/{MANIFEST}", base(), package.release());
    let text = agent(Duration::from_secs(30))
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .with_config()
        .limit(MAX_MANIFEST)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let manifest = Manifest::parse(&text).ok_or_else(|| format!("{url}: not a manifest"))?;
    if !newer(installed, &manifest.version) {
        return Ok(None);
    }
    let file = package.file().and_then(|name| manifest.file(name).cloned());
    Ok(Some(Offer {
        version: manifest.version,
        file,
        package,
    }))
}

/// Downloads `offer`'s file into `dir` and checks its size and SHA-256;
/// `progress` hears the share done. Returns the checked file.
pub fn download(offer: &Offer, dir: &Path, progress: &dyn Fn(f32)) -> Result<PathBuf, String> {
    let entry = offer.file.as_ref().ok_or("nothing to download")?;
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let target = dir.join(&entry.name);
    if verify(&target, entry).is_ok() {
        return Ok(target);
    }
    // Earlier downloads, and any left half done.
    if let Ok(old) = fs::read_dir(dir) {
        for file in old.flatten() {
            let _ = fs::remove_file(file.path());
        }
    }
    let part = dir.join(format!("{}.part", entry.name));
    let url = format!("{}/{}/{}", base(), offer.package.release(), entry.name);
    let mut answer = agent(Duration::from_secs(60 * 60))
        .get(&url)
        .call()
        .map_err(|e| e.to_string())?;
    let mut body = answer
        .body_mut()
        .with_config()
        .limit(entry.size + 1)
        .reader();
    let mut out = File::create(&part).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut done = 0u64;
    let mut buf = vec![0; 256 * 1024];
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > entry.size {
            break;
        }
        hash.update(&buf[..n]);
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress(done as f32 / entry.size.max(1) as f32);
    }
    out.sync_all().map_err(|e| e.to_string())?;
    drop(out);
    if done != entry.size || hex(&hash.finalize()) != entry.sha256 {
        let _ = fs::remove_file(&part);
        return Err(format!(
            "{}: not the file the release describes",
            entry.name
        ));
    }
    fs::rename(&part, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Checks a downloaded file against its entry again.
fn verify(file: &Path, entry: &Entry) -> io::Result<()> {
    let mut f = File::open(file)?;
    if f.metadata()?.len() != entry.size {
        return Err(io::Error::other("size"));
    }
    let mut hash = Sha256::new();
    io::copy(&mut f, &mut hash)?;
    if hex(&hash.finalize()) != entry.sha256 {
        return Err(io::Error::other("hash"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Why an update was not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The password prompt was closed, or the password refused.
    Cancelled,
    /// Installing failed; in the installer's words.
    Failed(String),
}

/// Installs the checked `file`. On Linux it returns once installed, and
/// [`restart`] starts the new Katna PDF; on Windows once Setup has
/// started, which closes this one and opens the new one itself.
pub fn install(offer: &Offer, file: &Path) -> Result<(), InstallError> {
    let entry = offer
        .file
        .as_ref()
        .ok_or(InstallError::Failed(String::new()))?;
    verify(file, entry).map_err(|e| InstallError::Failed(e.to_string()))?;
    let installed = match &offer.package {
        Package::Arch => {
            let out = Command::new("pkexec")
                .arg(ARCH_HELPER)
                .arg(file)
                .arg(&entry.sha256)
                .output()
                .map_err(|e| InstallError::Failed(format!("pkexec: {e}")))?;
            match out.status.code() {
                Some(0) => Ok(()),
                // pkexec: the prompt dismissed (126) or not authorised (127).
                Some(126 | 127) => Err(InstallError::Cancelled),
                _ => Err(InstallError::Failed(
                    String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                )),
            }
        }
        Package::AppImage(image) => {
            replace_appimage(file, image).map_err(|e| InstallError::Failed(e.to_string()))
        }
        Package::Windows { everyone } => run_setup(file, *everyone),
        Package::System | Package::Tarball | Package::WindowsZip => {
            Err(InstallError::Failed(String::new()))
        }
    };
    // Setup is still running from its file on Windows.
    if installed.is_ok() && !cfg!(windows) {
        let _ = fs::remove_file(file);
    }
    installed
}

/// Puts the new AppImage where the running one is, in one rename.
fn replace_appimage(file: &Path, image: &Path) -> io::Result<()> {
    let new = image.with_extension("AppImage.new");
    fs::copy(file, &new)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new, fs::Permissions::from_mode(0o755))?;
    }
    File::open(&new)?.sync_all()?;
    fs::rename(&new, image)
}

/// Runs the new Setup quietly from a hidden PowerShell outside the install
/// folder, which then opens Katna PDF again.
fn run_setup(setup: &Path, everyone: bool) -> Result<(), InstallError> {
    let exe = std::env::current_exe().map_err(|e| InstallError::Failed(e.to_string()))?;
    let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let (args, verb) = if everyone {
        ("'--quiet','--update','--all-users'", " -Verb RunAs")
    } else {
        ("'--quiet','--update'", "")
    };
    let script = format!(
        "$p = Start-Process -FilePath {} -ArgumentList {args}{verb} -Wait -PassThru; \
         Start-Process -FilePath {}",
        quote(setup),
        quote(&exe)
    );
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &script,
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .current_dir(std::env::temp_dir())
        .spawn()
        .map(drop)
        .map_err(|e| InstallError::Failed(format!("powershell: {e}")))
}

/// Starts the new Katna PDF with `files` once this one has closed; the
/// caller quits next. Not on Windows, where Setup does it.
pub fn restart(package: &Package, files: &[PathBuf]) -> io::Result<()> {
    if cfg!(windows) {
        return Ok(());
    }
    let exe = match package {
        Package::AppImage(image) => image.clone(),
        _ => PathBuf::from(SYSTEM_EXE),
    };
    Command::new("/bin/sh")
        .arg("-c")
        .arg(r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; shift; exec "$@""#)
        .arg("sh")
        .arg(std::process::id().to_string())
        .arg(exe)
        .args(files)
        .spawn()
        .map(drop)
}

/// What was last looked for, kept between runs in
/// `$XDG_STATE_HOME/katna/pdf/update.txt`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// When the last check ran, in seconds since 1970.
    pub checked: u64,
    /// The version "Not now" set aside: not offered again.
    pub skipped: Option<String>,
}

impl Seen {
    fn path() -> Option<PathBuf> {
        let paths = katna_core::Paths::from_env().ok()?;
        Some(paths.state_dir().join("pdf").join("update.txt"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    fn parse(text: &str) -> Self {
        let mut seen = Self::default();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("checked", v)) => seen.checked = v.trim().parse().unwrap_or(0),
                Some(("skipped", v)) => seen.skipped = Some(v.trim().to_owned()),
                _ => {}
            }
        }
        seen
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        let mut text = format!("checked {}\n", self.checked);
        if let Some(v) = &self.skipped {
            text.push_str(&format!("skipped {v}\n"));
        }
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(path, text);
    }

    /// Whether it is time to look again.
    pub fn due(&self) -> bool {
        now().saturating_sub(self.checked) >= EVERY.as_secs()
    }

    pub fn mark_checked(&mut self) {
        self.checked = now();
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Where downloads wait to be installed.
pub fn download_dir() -> Option<PathBuf> {
    let paths = katna_core::Paths::from_env().ok()?;
    Some(paths.cache_dir().join("pdf").join("update"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn versions_order_by_commits() {
        assert!(newer("0.0.0.r500.gabc1234", "0.0.0.r501.g0000000"));
        assert!(!newer("0.0.0.r501.gabc1234", "0.0.0.r500.g0000000"));
        assert!(!newer("0.0.0.r500.gabc1234", "0.0.0.r500.gdef5678"));
        assert!(newer("0.0.0.r900.gabc1234", "0.1.0"));
        assert!(newer("0.1.0", "0.1.0.r1.gabc1234"));
        assert!(!newer("0.1.0", "rubbish"));
        assert!(!newer("0.1.0", "0.1.0.r1"));
        assert!(!newer("0.1.0", "0.1.0.r1.gxyz"));
    }

    #[test]
    fn manifests_are_read_strictly() {
        let text = format!(
            "version 0.0.0.r12.gabc1234\nfile {SHA} 1024 katna-pdf-git-x86_64.pkg.tar.zst\n\n"
        );
        let m = Manifest::parse(&text).unwrap();
        assert_eq!(m.version, "0.0.0.r12.gabc1234");
        assert_eq!(
            m.file("katna-pdf-git-x86_64.pkg.tar.zst").unwrap().size,
            1024
        );
        for bad in [
            format!("file {SHA} 1 a.zst\n"),
            format!("version 1.2\nfile {SHA} 1 a.zst\n"),
            format!("version 1.2.3\nfile {SHA} 1 ../a.zst\n"),
            format!("version 1.2.3\nfile {SHA} 1 .hidden\n"),
            format!("version 1.2.3\nfile {} 1 a.zst\n", &SHA[1..]),
            format!("version 1.2.3\nfile {SHA} 1 a.zst extra\n"),
            "version 1.2.3\nsomething else\n".to_owned(),
        ] {
            assert_eq!(Manifest::parse(&bad), None, "{bad}");
        }
    }

    #[test]
    fn packages_find_their_files() {
        assert_eq!(Package::Arch.release(), "arch-latest");
        assert_eq!(
            Package::Arch.file(),
            Some("katna-pdf-git-x86_64.pkg.tar.zst")
        );
        assert_eq!(Package::System.file(), None);
        assert!(
            Package::WindowsZip
                .page()
                .ends_with("/releases/tag/windows-latest")
        );
    }

    #[test]
    fn seen_round_trips() {
        let seen = Seen::parse("checked 42\nskipped 0.0.0.r3.gabc\n");
        assert_eq!(seen.checked, 42);
        assert_eq!(seen.skipped.as_deref(), Some("0.0.0.r3.gabc"));
        assert!(seen.due());
    }

    #[test]
    fn downloads_are_checked() {
        let dir = std::env::temp_dir().join(format!("katna-pdf-update-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.bin");
        fs::write(&file, b"hello").unwrap();
        let good = Entry {
            name: "a.bin".into(),
            sha256: hex(&Sha256::digest(b"hello")),
            size: 5,
        };
        assert!(verify(&file, &good).is_ok());
        assert!(
            verify(
                &file,
                &Entry {
                    size: 4,
                    ..good.clone()
                }
            )
            .is_err()
        );
        assert!(
            verify(
                &file,
                &Entry {
                    sha256: SHA.into(),
                    ..good
                }
            )
            .is_err()
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
