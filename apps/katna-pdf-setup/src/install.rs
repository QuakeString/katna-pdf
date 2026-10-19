// SPDX-License-Identifier: GPL-3.0-or-later

//! What Setup does: installs Katna PDF for the current user into
//! `%LOCALAPPDATA%\Programs\Katna PDF` without an administrator prompt, or
//! for everyone into `%ProgramFiles%\Katna PDF` (Windows asks for an
//! administrator once), and removes it again. It is Katna Setup's
//! (Katna's `apps/katna-setup`) way of installing, for Katna PDF.
//!
//! Installing closes a running Katna PDF, unpacks the payload, copies
//! Setup in as `katna-pdf-setup.exe` (Settings > Apps runs it to
//! uninstall) and adds Katna PDF to Windows: the shortcuts the user chose,
//! the uninstall entry, `katna-pdf` for Run, and, if chosen, Katna PDF in
//! Open with and Default apps for PDF files. A per-user install writes
//! only the user's own profile and registry; an install for everyone
//! writes the machine's.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::payload::Payload;

/// The steps [`install`] reports, in order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// Closing a running Katna PDF.
    Stopping,
    /// Unpacking files; the share done so far.
    Copying(f32),
    /// Adding Katna PDF to Windows.
    Registering,
}

/// The app, as the payload has it.
pub const APP_EXE: &str = "katna-pdf.exe";
/// Setup's own copy, for uninstalling.
pub const SETUP_EXE: &str = "katna-pdf-setup.exe";
/// The icon, for the uninstall entry and PDF files.
#[cfg_attr(not(windows), allow(dead_code))]
pub const ICON: &str = "katna-pdf.ico";
/// The programs folder's own name.
const FOLDER: &str = "Katna PDF";
/// The shortcuts' file name.
const LINK: &str = "Katna PDF.lnk";
/// The name Windows lists Katna PDF under in Default apps.
#[cfg_attr(not(windows), allow(dead_code))]
pub const REGISTERED_NAME: &str = "Katna PDF";

/// Who Katna PDF is installed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The current user only; no administrator needed.
    User,
    /// Everyone on the computer; needs an administrator.
    Machine,
}

/// What the user chose to add to Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choices {
    pub desktop: bool,
    pub start_menu: bool,
    /// Katna PDF in Open with and Default apps for PDF files.
    pub pdf: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self {
            desktop: false,
            start_menu: true,
            pdf: true,
        }
    }
}

/// Where Katna PDF is installed and where Windows looks for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub scope: Scope,
    /// The programs: by default `%LOCALAPPDATA%\Programs\Katna PDF` for
    /// one user, `%ProgramFiles%\Katna PDF` for everyone.
    pub programs: PathBuf,
    /// The Start menu's programs folder.
    pub start_menu: PathBuf,
    /// The desktop the shortcut goes on: the user's, or the public one.
    pub desktop: PathBuf,
}

impl Layout {
    /// The default layout for `scope`, from the environment.
    pub fn from_env(scope: Scope) -> io::Result<Self> {
        Self::from_lookup(scope, |name| std::env::var_os(name))
    }

    pub fn from_lookup(
        scope: Scope,
        lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> io::Result<Self> {
        let folder = |name: &str| {
            lookup(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .ok_or_else(|| io::Error::other(format!("%{name}% is not set")))
        };
        let start_menu = |root: PathBuf| {
            root.join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
        };
        Ok(match scope {
            Scope::User => Self {
                scope,
                programs: folder("LOCALAPPDATA")?.join("Programs").join(FOLDER),
                start_menu: start_menu(folder("APPDATA")?),
                desktop: folder("USERPROFILE")?.join("Desktop"),
            },
            Scope::Machine => Self {
                scope,
                programs: folder("ProgramFiles")?.join(FOLDER),
                start_menu: start_menu(folder("ProgramData")?),
                desktop: folder("PUBLIC")?.join("Desktop"),
            },
        })
    }

    /// This layout with the programs in `dir`: a folder named Katna PDF,
    /// or a new Katna PDF folder inside `dir`, so removing Katna PDF never
    /// deletes a folder the user picked.
    pub fn in_folder(mut self, dir: &Path) -> Self {
        self.programs = if dir
            .file_name()
            .is_some_and(|n| n.eq_ignore_ascii_case(FOLDER))
        {
            dir.to_owned()
        } else {
            dir.join(FOLDER)
        };
        self
    }

    /// The Start menu shortcut.
    pub fn shortcut(&self) -> PathBuf {
        self.start_menu.join(LINK)
    }

    /// The desktop shortcut.
    pub fn desktop_shortcut(&self) -> PathBuf {
        self.desktop.join(LINK)
    }

    /// Whether Katna PDF is installed here already.
    pub fn installed(&self) -> bool {
        self.programs.join(APP_EXE).is_file()
    }

    /// Whether the programs folder can take Katna PDF: a new folder, or
    /// one that holds Katna PDF already, is empty or holds only a Setup
    /// left by an uninstall, and not a link to somewhere else. Setup never
    /// mixes Katna PDF into another program's files, which removing it
    /// would delete.
    pub fn check_folder(&self) -> io::Result<()> {
        let dir = &self.programs;
        let usable = match std::fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => false,
            Ok(_) => self.installed() || only_setup_left(dir)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => true,
            Err(err) => return Err(err),
        };
        let path = dir.display().to_string();
        if !usable {
            return Err(io::Error::other(crate::text::t(
                "setup-folder-not-empty",
                &[("path", &path)],
            )));
        }
        // For everyone, outside Program Files: the folders above Katna
        // PDF's must be safe from other users too, or one could rename
        // them and put their own Katna PDF where every user opens PDFs
        // with it. An update of an existing install is let through.
        if self.scope == Scope::Machine && !self.in_program_files() && !self.installed() {
            let plain = dir
                .components()
                .all(|c| !matches!(c, Component::ParentDir | Component::CurDir));
            let parent = dir.parent().filter(|p| p.is_dir());
            if !plain || !parent.is_some_and(only_admins_change) {
                return Err(io::Error::other(crate::text::t(
                    "setup-folder-unsafe",
                    &[("path", &path)],
                )));
            }
        }
        Ok(())
    }

    /// Whether the programs are inside `%ProgramFiles%`, whose rules
    /// already keep users from changing them.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn in_program_files(&self) -> bool {
        let lower = |p: &Path| p.display().to_string().to_lowercase();
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .is_some_and(|root| {
                let (root, dir) = (lower(&root), lower(&self.programs));
                dir.strip_prefix(root.trim_end_matches('\\'))
                    .is_some_and(|rest| rest.starts_with('\\'))
            })
    }

    /// The shortcuts and PDF files as they are now, for an update to keep.
    pub fn current_choices(&self) -> Choices {
        Choices {
            desktop: self.desktop_shortcut().is_file(),
            start_menu: self.shortcut().is_file(),
            pdf: opens_pdfs(self.scope),
        }
    }

    /// Katna PDF as installed on this computer: where the uninstall entry
    /// says, for this user first, then for everyone.
    pub fn existing() -> Option<Self> {
        for scope in [Scope::User, Scope::Machine] {
            let Ok(layout) = Self::from_env(scope) else {
                continue;
            };
            let layout = match install_location(scope) {
                Some(dir) => Self {
                    programs: dir,
                    ..layout
                },
                None => layout,
            };
            if layout.installed() {
                return Some(layout);
            }
        }
        None
    }
}

/// The folder the uninstall entry for `scope` names.
#[cfg(windows)]
fn install_location(scope: Scope) -> Option<PathBuf> {
    root(scope)
        .open_subkey(UNINSTALL_KEY)
        .and_then(|key| key.get_value::<String, _>("InstallLocation"))
        .ok()
        .map(PathBuf::from)
}

#[cfg(not(windows))]
fn install_location(_scope: Scope) -> Option<PathBuf> {
    None
}

/// The registry Katna PDF's entries go in for `scope`.
#[cfg(all(windows, not(test)))]
fn root(scope: Scope) -> winreg::RegKey {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    winreg::RegKey::predef(match scope {
        Scope::User => HKEY_CURRENT_USER,
        Scope::Machine => HKEY_LOCAL_MACHINE,
    })
}

/// In tests, a key of each test's own under the user's, for both scopes:
/// tests never touch the real entries (a developer's own Katna PDF), and
/// tests running at once never delete keys another is writing.
#[cfg(all(windows, test))]
fn root(scope: Scope) -> winreg::RegKey {
    let (key, _) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .create_subkey(format!(r"{}\{scope:?}", tests::registry()))
        .expect("test registry key");
    key
}

/// Katna PDF's kind of file: what PDFs opened with it are called and
/// shown as.
#[cfg_attr(not(windows), allow(dead_code))]
const PROG_ID: &str = "KatnaPDF.Document";
/// Where Default apps finds what Katna PDF can open.
#[cfg_attr(not(windows), allow(dead_code))]
const CAPABILITIES_KEY: &str = r"Software\KatnaPDF\Capabilities";
#[cfg_attr(not(windows), allow(dead_code))]
const OPEN_WITH_KEY: &str = r"Software\Classes\.pdf\OpenWithProgids";

/// Whether Katna PDF is in Open with for PDFs.
#[cfg(windows)]
fn opens_pdfs(scope: Scope) -> bool {
    root(scope)
        .open_subkey(OPEN_WITH_KEY)
        .and_then(|key| key.get_raw_value(PROG_ID))
        .is_ok()
}

#[cfg(not(windows))]
fn opens_pdfs(_scope: Scope) -> bool {
    false
}

/// Whether this process may write the machine's registry and Program
/// Files, as an install for everyone must.
#[cfg(windows)]
pub fn elevated() -> bool {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_WRITE};
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("SOFTWARE", KEY_WRITE)
        .is_ok()
}

#[cfg(not(windows))]
pub fn elevated() -> bool {
    true
}

/// Runs this Setup again with `args`, as an administrator: Windows asks
/// the user first. Returns its exit code; an error when the user said no.
#[cfg(windows)]
pub fn run_elevated(args: &[String]) -> io::Result<i32> {
    use std::os::windows::process::CommandExt;
    let me = std::env::current_exe()?;
    let list = args
        .iter()
        .map(|a| format!("\"{a}\""))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "$p = Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs -Wait -PassThru; \
         exit $p.ExitCode",
        ps_quote(&me.display().to_string()),
        ps_quote(&list)
    );
    let status = std::process::Command::new(system_program(POWERSHELL))
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    status
        .code()
        .ok_or_else(|| io::Error::other("Setup stopped"))
}

#[cfg(not(windows))]
pub fn run_elevated(_args: &[String]) -> io::Result<i32> {
    Err(io::Error::other("only on Windows"))
}

/// Installs `payload` into `layout`, calling `progress` as it goes.
/// With `choices`, the shortcuts and PDF files are set as chosen;
/// without, an update keeps them as they are.
pub fn install(
    layout: &Layout,
    payload: &Payload<'_>,
    version: &str,
    choices: Option<Choices>,
    progress: &dyn Fn(Step),
) -> io::Result<()> {
    let choices = choices.unwrap_or_else(|| layout.current_choices());
    layout.check_folder()?;
    progress(Step::Stopping);
    stop(&layout.programs);
    std::fs::create_dir_all(&layout.programs)?;
    if layout.scope == Scope::Machine && !layout.in_program_files() {
        lock(&layout.programs)?;
    }
    unpack(&layout.programs, payload, &|share| {
        progress(Step::Copying(share))
    })?;
    progress(Step::Registering);
    // Setup copies itself in; it is not in the payload.
    if let Ok(me) = std::env::current_exe()
        && me != layout.programs.join(SETUP_EXE)
    {
        std::fs::copy(me, layout.programs.join(SETUP_EXE))?;
    }
    register(layout, payload, version, choices)
}

/// Unpacks every file of `payload` into `dir`, a file at a time: each is
/// written beside its old copy and then put in its place.
pub fn unpack(dir: &Path, payload: &Payload<'_>, progress: &dyn Fn(f32)) -> io::Result<()> {
    let total = payload.size().max(1) as f32;
    let mut done = 0u64;
    for entry in &payload.entries {
        let path = dir.join(entry.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let partial = path.with_extension("katna-new");
        {
            let mut file = io::BufWriter::new(std::fs::File::create(&partial)?);
            entry.unpack(&mut file)?;
            io::Write::flush(&mut file)?;
        }
        replace(&partial, &path)?;
        done += entry.size;
        progress(done as f32 / total);
    }
    Ok(())
}

/// Moves `new` over `path`, trying again for a moment while a closing
/// program still holds it.
fn replace(new: &Path, path: &Path) -> io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(new, path) {
            Ok(()) => return Ok(()),
            Err(_) if tries < 20 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            Err(err) => {
                let _ = std::fs::remove_file(new);
                return Err(io::Error::new(
                    err.kind(),
                    format!("{}: {err}", path.display()),
                ));
            }
        }
    }
}

/// Removes Katna PDF from `layout`; with `data`, also the user's settings,
/// recent files and library on this computer.
pub fn uninstall(layout: &Layout, data: bool) -> io::Result<()> {
    stop(&layout.programs);
    unregister(layout);
    let _ = std::fs::remove_file(layout.shortcut());
    let _ = std::fs::remove_file(layout.desktop_shortcut());
    if data {
        delete_data()?;
    }
    remove_programs(&layout.programs)
}

/// Deletes this user's Katna PDF settings, recent files and library: the
/// `pdf` folders in Katna's folders, never Katna Mail's.
pub fn delete_data() -> io::Result<()> {
    if let Ok(paths) = katna_core::paths::Paths::from_env() {
        for dir in [
            paths.config_dir(),
            paths.data_dir(),
            paths.cache_dir(),
            paths.state_dir(),
        ] {
            match std::fs::remove_dir_all(dir.join("pdf")) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Closes every program running from `programs`: the app and the
/// workers it draws pages in.
#[cfg(windows)]
fn stop(programs: &Path) {
    use std::os::windows::process::CommandExt;
    if !programs.is_dir() {
        return;
    }
    let script = format!(
        "{} | Where-Object {{ $_.Id -ne {me} }} | Stop-Process -Force",
        running_from(programs),
        me = std::process::id()
    );
    let stopped = std::process::Command::new(system_program(POWERSHELL))
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    if let Err(err) = stopped {
        eprintln!("katna-pdf-setup: cannot close the running Katna PDF: {err}");
    }
}

#[cfg(not(windows))]
fn stop(_programs: &Path) {}

/// Gives a folder for everyone outside Program Files the same rules as
/// Program Files: administrators and Windows own it and can change it,
/// users can only read and run it. Otherwise any user could replace the
/// program every other user opens PDFs with.
#[cfg(windows)]
fn lock(dir: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Administrators, SYSTEM and Users by their well-known SIDs, which
    // every language of Windows has.
    let steps: [&[&str]; 2] = [
        &["/setowner", "*S-1-5-32-544", "/T", "/C", "/Q"],
        &[
            "/inheritance:r",
            "/grant:r",
            "*S-1-5-32-544:(OI)(CI)F",
            "*S-1-5-18:(OI)(CI)F",
            "*S-1-5-32-545:(OI)(CI)RX",
            "/T",
            "/C",
            "/Q",
        ],
    ];
    for args in steps {
        let status = std::process::Command::new(system_program("icacls.exe"))
            .arg(dir)
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "{}: icacls {status}",
                dir.display()
            )));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn lock(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Whether only administrators and Windows can rename, delete or take
/// over `dir` and every folder above it: each is owned by Administrators,
/// SYSTEM or TrustedInstaller, and gives nobody else Delete, Delete
/// subfolders and files, Change permissions or Take ownership (Delete on
/// a drive's root does not matter: it cannot be renamed). Anything it
/// cannot read counts as unsafe.
#[cfg(windows)]
fn only_admins_change(dir: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    let script = format!(
        "$ErrorActionPreference = 'Stop'
$sid = [System.Security.Principal.SecurityIdentifier]
$safe = @('S-1-5-32-544', 'S-1-5-18', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
$d = Get-Item -LiteralPath '{}' -Force
while ($d) {{
  $acl = Get-Acl -LiteralPath $d.FullName
  if ($safe -notcontains $acl.GetOwner($sid).Value) {{ exit 3 }}
  $bad = [int64]0x500C0040
  if ($d.Parent) {{ $bad = $bad -bor 0x10000 }}
  foreach ($r in $acl.GetAccessRules($true, $true, $sid)) {{
    if ($r.AccessControlType -ne 'Allow') {{ continue }}
    if (([int]$r.PropagationFlags -band 2) -ne 0) {{ continue }}
    if ($safe -contains $r.IdentityReference.Value) {{ continue }}
    if (([int64]$r.FileSystemRights -band $bad) -ne 0) {{ exit 3 }}
  }}
  $d = $d.Parent
}}
exit 0",
        ps_quote(&dir.display().to_string())
    );
    // A script of several lines goes as UTF-16 Base64, as PowerShell reads it.
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, utf16);
    std::process::Command::new(system_program(POWERSHELL))
        .args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded])
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(windows))]
fn only_admins_change(_dir: &Path) -> bool {
    true
}

/// Where a Setup running as administrator puts how far it got, for the
/// Setup that started it to read: a key only administrators can change,
/// rather than a file the caller names, which Setup as administrator
/// would overwrite wherever it is.
#[cfg_attr(not(windows), allow(dead_code))]
const PROGRESS_KEY: &str = r"SOFTWARE\Katna\PdfSetup";

#[cfg_attr(not(windows), allow(dead_code))]
fn progress_value(id: u32) -> String {
    format!("Progress-{id}")
}

/// Records `line` for the Setup with process ID `id`.
#[cfg(windows)]
pub fn write_progress(id: u32, line: &str) {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    if let Ok((key, _)) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(PROGRESS_KEY) {
        let _ = key.set_value(progress_value(id), &line.to_owned());
    }
}

#[cfg(not(windows))]
pub fn write_progress(_id: u32, _line: &str) {}

/// What the Setup running as administrator recorded for this one, `id`.
#[cfg(windows)]
pub fn read_progress(id: u32) -> Option<String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(PROGRESS_KEY)
        .and_then(|key| key.get_value::<String, _>(progress_value(id)))
        .ok()
}

#[cfg(not(windows))]
pub fn read_progress(_id: u32) -> Option<String> {
    None
}

/// Removes what [`write_progress`] recorded, and the keys once empty.
#[cfg(windows)]
pub fn clear_progress(id: u32) {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    let machine = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    if let Ok(key) = machine.open_subkey_with_flags(PROGRESS_KEY, KEY_SET_VALUE) {
        let _ = key.delete_value(progress_value(id));
    }
    // Windows deletes a key with values in it, so only empty ones; Katna
    // Mail's Setup shares SOFTWARE\Katna.
    for path in [PROGRESS_KEY, r"SOFTWARE\Katna"] {
        let empty = machine.open_subkey(path).is_ok_and(|key| {
            key.enum_values().next().is_none() && key.enum_keys().next().is_none()
        });
        if empty {
            let _ = machine.delete_subkey(path);
        }
    }
}

#[cfg(not(windows))]
pub fn clear_progress(_id: u32) {}

/// The uninstall entry in Settings > Apps.
#[cfg_attr(not(windows), allow(dead_code))]
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\KatnaPDF";
/// What lets Run (Windows+R) start `katna-pdf`.
#[cfg_attr(not(windows), allow(dead_code))]
const APP_PATH_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\App Paths\katna-pdf.exe";

#[cfg(windows)]
fn register(
    layout: &Layout,
    payload: &Payload<'_>,
    version: &str,
    choices: Choices,
) -> io::Result<()> {
    use crate::text::t;

    let root = root(layout.scope);
    let dir = &layout.programs;
    let app = dir.join(APP_EXE);
    let icon = dir.join(ICON);
    let setup = dir.join(SETUP_EXE);

    let link = |path: PathBuf, on: bool| -> io::Result<()> {
        if !on {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut link = mslnk::ShellLink::new(&app).map_err(io::Error::other)?;
        link.set_working_dir(Some(dir.display().to_string()));
        link.set_icon_location(Some(icon.display().to_string()));
        link.create_lnk(path).map_err(io::Error::other)
    };
    link(layout.shortcut(), choices.start_menu)?;
    link(layout.desktop_shortcut(), choices.desktop)?;

    let (key, _) = root.create_subkey(UNINSTALL_KEY)?;
    let name = t("setup-app-name", &[]);
    key.set_value("DisplayName", &name)?;
    key.set_value("DisplayVersion", &version)?;
    key.set_value("Publisher", &"Katna")?;
    key.set_value("DisplayIcon", &icon.display().to_string())?;
    key.set_value("InstallLocation", &dir.display().to_string())?;
    let everyone = match layout.scope {
        Scope::User => "",
        Scope::Machine => " --all-users",
    };
    key.set_value(
        "UninstallString",
        &format!("\"{}\" --uninstall{everyone}", setup.display()),
    )?;
    key.set_value(
        "QuietUninstallString",
        &format!("\"{}\" --uninstall --quiet{everyone}", setup.display()),
    )?;
    key.set_value("URLInfoAbout", &"https://github.com/QuakeString/katna-pdf")?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    let kb = u32::try_from(payload.size() / 1024).unwrap_or(u32::MAX);
    key.set_value("EstimatedSize", &kb)?;

    let (key, _) = root.create_subkey(APP_PATH_KEY)?;
    key.set_value("", &app.display().to_string())?;
    key.set_value("Path", &dir.display().to_string())?;

    if choices.pdf {
        register_pdf(&root, &app, &icon)?;
    } else {
        unregister_pdf(&root);
    }
    Ok(())
}

/// Katna PDF in Open with for PDF files, and in Settings > Apps > Default
/// apps, where the user makes it the app for PDFs: Windows 10 and later
/// let only the user choose the default.
#[cfg(windows)]
fn register_pdf(root: &winreg::RegKey, app: &Path, icon: &Path) -> io::Result<()> {
    use crate::text::t;

    let open = format!("\"{}\" \"%1\"", app.display());
    let kind = t("setup-pdf-kind", &[]);
    let (key, _) = root.create_subkey(format!(r"Software\Classes\{PROG_ID}"))?;
    key.set_value("", &kind)?;
    key.set_value("FriendlyTypeName", &kind)?;
    let (key, _) = root.create_subkey(format!(r"Software\Classes\{PROG_ID}\DefaultIcon"))?;
    key.set_value("", &format!("\"{}\",0", icon.display()))?;
    let (key, _) = root.create_subkey(format!(r"Software\Classes\{PROG_ID}\shell\open\command"))?;
    key.set_value("", &open)?;

    let (key, _) = root.create_subkey(OPEN_WITH_KEY)?;
    key.set_value(PROG_ID, &"")?;

    let apps = format!(r"Software\Classes\Applications\{APP_EXE}");
    let (key, _) = root.create_subkey(&apps)?;
    key.set_value("FriendlyAppName", &t("setup-app-name", &[]))?;
    let (key, _) = root.create_subkey(format!(r"{apps}\SupportedTypes"))?;
    key.set_value(".pdf", &"")?;
    let (key, _) = root.create_subkey(format!(r"{apps}\shell\open\command"))?;
    key.set_value("", &open)?;

    let (key, _) = root.create_subkey(CAPABILITIES_KEY)?;
    key.set_value("ApplicationName", &t("setup-app-name", &[]))?;
    key.set_value("ApplicationDescription", &t("setup-tagline", &[]))?;
    key.set_value("ApplicationIcon", &format!("\"{}\",0", icon.display()))?;
    let (key, _) = root.create_subkey(format!(r"{CAPABILITIES_KEY}\FileAssociations"))?;
    key.set_value(".pdf", &PROG_ID)?;
    let (key, _) = root.create_subkey(r"Software\RegisteredApplications")?;
    key.set_value(REGISTERED_NAME, &CAPABILITIES_KEY)?;
    Ok(())
}

#[cfg(windows)]
fn unregister_pdf(root: &winreg::RegKey) {
    use winreg::enums::KEY_SET_VALUE;
    for key in [
        format!(r"Software\Classes\{PROG_ID}"),
        format!(r"Software\Classes\Applications\{APP_EXE}"),
        r"Software\KatnaPDF".to_owned(),
    ] {
        let _ = root.delete_subkey_all(key);
    }
    if let Ok(key) = root.open_subkey_with_flags(OPEN_WITH_KEY, KEY_SET_VALUE) {
        let _ = key.delete_value(PROG_ID);
    }
    if let Ok(key) = root.open_subkey_with_flags(r"Software\RegisteredApplications", KEY_SET_VALUE)
    {
        let _ = key.delete_value(REGISTERED_NAME);
    }
}

#[cfg(not(windows))]
fn register(
    layout: &Layout,
    _payload: &Payload<'_>,
    _version: &str,
    choices: Choices,
) -> io::Result<()> {
    // Only the shortcuts' places, so tests can check them on Linux.
    for (path, on) in [
        (layout.shortcut(), choices.start_menu),
        (layout.desktop_shortcut(), choices.desktop),
    ] {
        if on {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, b"")?;
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn unregister(layout: &Layout) {
    let root = root(layout.scope);
    unregister_pdf(&root);
    for key in [UNINSTALL_KEY, APP_PATH_KEY] {
        let _ = root.delete_subkey_all(key);
    }
}

#[cfg(not(windows))]
fn unregister(_layout: &Layout) {}

/// Opens Settings > Apps > Default apps at Katna PDF (Windows 11), or at
/// Default apps (Windows 10), where the user can make it the app for PDFs.
#[cfg(windows)]
pub fn open_default_apps(scope: Scope) {
    let which = match scope {
        Scope::User => "registeredAppUser",
        Scope::Machine => "registeredAppMachine",
    };
    let uri = format!(
        "ms-settings:defaultapps?{which}={}",
        REGISTERED_NAME.replace(' ', "%20")
    );
    let explorer = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("explorer.exe");
    if let Err(err) = std::process::Command::new(explorer).arg(uri).spawn() {
        eprintln!("katna-pdf-setup: cannot open Default apps: {err}");
    }
}

#[cfg(not(windows))]
pub fn open_default_apps(_scope: Scope) {}

/// Deletes the programs folder. When Windows' Apps settings uninstall
/// Katna PDF, Setup's own copy in that folder is running, and for
/// everyone the user's Setup waits there for the administrator's, so a
/// PowerShell removes the folder once no program runs from it any more.
fn remove_programs(programs: &Path) -> io::Result<()> {
    // Only Katna PDF's own folder: never one without Katna PDF in it.
    if !programs.join(APP_EXE).is_file() {
        return Ok(());
    }
    let running_here = std::env::current_exe()
        .ok()
        .is_some_and(|me| me.starts_with(programs));
    if !running_here {
        return match std::fs::remove_dir_all(programs) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let script = format!(
            "$until = (Get-Date).AddHours(1); \
             while ((Get-Date) -lt $until -and ({running})) {{ Start-Sleep -Seconds 1 }}; \
             foreach ($try in 1..10) {{ \
             Start-Sleep -Seconds 1; \
             Remove-Item -LiteralPath '{folder}' -Recurse -Force -ErrorAction SilentlyContinue; \
             if (-not (Test-Path -LiteralPath '{folder}')) {{ break }} }}",
            running = running_from(programs),
            folder = ps_quote(&programs.display().to_string())
        );
        std::process::Command::new(system_program(POWERSHELL))
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
    }
    Ok(())
}

/// A PowerShell pipeline listing the processes started from `folder`.
#[cfg_attr(not(windows), allow(dead_code))]
fn running_from(folder: &Path) -> String {
    format!(
        "Get-Process | Where-Object {{ $_.Path -and $_.Path.StartsWith('{}\\', \
         [StringComparison]::OrdinalIgnoreCase) }}",
        ps_quote(&folder.display().to_string())
    )
}

/// `s` inside a single-quoted PowerShell string. PowerShell also ends such
/// a string at the typographic single quotes, so those are doubled too.
#[cfg_attr(not(windows), allow(dead_code))]
fn ps_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out
}

/// PowerShell, under `%SystemRoot%\System32`.
#[cfg_attr(not(windows), allow(dead_code))]
const POWERSHELL: &str = r"WindowsPowerShell\v1.0\powershell.exe";

/// A program of Windows by its full path under `%SystemRoot%\System32`.
/// By name alone, Windows would look in Setup's own folder first, often
/// Downloads, where any page can leave a `powershell.exe` that would then
/// run as administrator.
#[cfg_attr(not(windows), allow(dead_code))]
fn system_program(name: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join(name)
}

/// Whether `dir` is empty or holds nothing but `katna-pdf-setup.exe`:
/// what an uninstall leaves when the Setup it ran could not remove
/// itself. Installing puts its own copy there.
fn only_setup_left(dir: &Path) -> io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let setup = entry.file_name().eq_ignore_ascii_case(SETUP_EXE);
        if !setup || !entry.file_type()?.is_file() {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    /// Where this test's registry entries go: one key per test, as each
    /// runs on its own thread.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(super) fn registry() -> String {
        format!(
            r"Software\Katna\PdfTests\{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        )
    }

    /// Deletes this test's registry entries when the test ends.
    struct Registry;

    impl Drop for Registry {
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                let user = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
                let _ = user.delete_subkey_all(registry());
            }
        }
    }

    /// An absolute path on this system: Windows needs a drive.
    fn at(path: &str) -> PathBuf {
        Path::new(if cfg!(windows) { "C:\\" } else { "/" }).join(path)
    }

    fn windows(name: &str) -> Option<OsString> {
        let path = match name {
            "LOCALAPPDATA" => "Users/ada/AppData/Local",
            "APPDATA" => "Users/ada/AppData/Roaming",
            "USERPROFILE" => "Users/ada",
            "ProgramFiles" => "Program Files",
            "ProgramData" => "ProgramData",
            "PUBLIC" => "Users/Public",
            _ => return None,
        };
        Some(at(path).into_os_string())
    }

    fn payload_of(files: &[(&str, &[u8])]) -> Vec<u8> {
        let source = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            std::fs::write(source.path().join(name), bytes).unwrap();
        }
        let mut bytes = Vec::new();
        crate::payload::write(source.path(), &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn finds_only_programs_inside_the_folder() {
        let script = running_from(Path::new(r"C:\Kat'na\Katna PDF"));
        assert!(script.contains(r"StartsWith('C:\Kat''na\Katna PDF\', "));
    }

    #[test]
    fn installs_under_the_users_own_folders() {
        let layout = Layout::from_lookup(Scope::User, windows).unwrap();
        assert_eq!(
            layout.programs,
            at("Users/ada/AppData/Local/Programs/Katna PDF")
        );
        assert_eq!(
            layout.shortcut(),
            at("Users/ada/AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Katna PDF.lnk")
        );
        assert_eq!(
            layout.desktop_shortcut(),
            at("Users/ada/Desktop/Katna PDF.lnk")
        );
        assert!(Layout::from_lookup(Scope::User, |_| None).is_err());
    }

    #[test]
    fn installs_for_everyone_in_the_machines_folders() {
        let layout = Layout::from_lookup(Scope::Machine, windows).unwrap();
        assert_eq!(layout.programs, at("Program Files/Katna PDF"));
        assert!(
            layout
                .shortcut()
                .starts_with(at("ProgramData/Microsoft/Windows/Start Menu"))
        );
        assert_eq!(
            layout.desktop_shortcut(),
            at("Users/Public/Desktop/Katna PDF.lnk")
        );
    }

    #[test]
    fn a_picked_folder_gets_its_own_katna_pdf_folder() {
        let layout = Layout::from_lookup(Scope::User, windows).unwrap();
        assert_eq!(
            layout.clone().in_folder(&at("D/Apps")).programs,
            at("D/Apps/Katna PDF")
        );
        assert_eq!(
            layout.in_folder(&at("D/Apps/katna pdf")).programs,
            at("D/Apps/katna pdf")
        );
    }

    #[test]
    fn shortcuts_and_pdf_files_follow_the_choices() {
        let _registry = Registry;
        let root = tempfile::tempdir().unwrap();
        let bytes = payload_of(&[(APP_EXE, b"pdf"), (ICON, b"icon")]);
        let payload = Payload::read(&bytes).unwrap();
        let layout = Layout {
            scope: Scope::User,
            programs: root.path().join("Programs/Katna PDF"),
            start_menu: root.path().join("Start Menu"),
            desktop: root.path().join("Desktop"),
        };
        let choices = Choices {
            desktop: true,
            start_menu: false,
            pdf: cfg!(windows),
        };
        install(&layout, &payload, "1", Some(choices), &|_| {}).unwrap();
        assert!(layout.desktop_shortcut().is_file());
        assert!(!layout.shortcut().exists());
        // An update keeps them.
        install(&layout, &payload, "2", None, &|_| {}).unwrap();
        assert_eq!(layout.current_choices(), choices);
        uninstall(&layout, false).unwrap();
        assert!(!layout.desktop_shortcut().exists());
        assert!(!layout.current_choices().pdf);
    }

    #[test]
    fn unpacks_and_replaces_files() {
        let bytes = payload_of(&[(APP_EXE, b"new")]);
        let payload = Payload::read(&bytes).unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(target.path().join(APP_EXE), b"old").unwrap();
        let shares = std::cell::RefCell::new(Vec::new());
        unpack(target.path(), &payload, &|share| {
            shares.borrow_mut().push(share)
        })
        .unwrap();
        assert_eq!(std::fs::read(target.path().join(APP_EXE)).unwrap(), b"new");
        assert_eq!(*shares.borrow(), [1.0]);
        assert!(!target.path().join("katna-pdf.katna-new").exists());
    }

    #[test]
    fn uninstall_removes_the_programs() {
        let _registry = Registry;
        let root = tempfile::tempdir().unwrap();
        let layout = Layout {
            scope: Scope::User,
            programs: root.path().join("Programs/Katna PDF"),
            start_menu: root.path().join("Start Menu"),
            desktop: root.path().join("Desktop"),
        };
        std::fs::create_dir_all(&layout.programs).unwrap();
        std::fs::write(layout.programs.join(APP_EXE), b"x").unwrap();
        assert!(layout.installed());
        uninstall(&layout, false).unwrap();
        assert!(!layout.programs.exists());
    }

    #[test]
    fn powershell_strings_keep_every_quote() {
        assert_eq!(ps_quote("C:\\Bob's"), "C:\\Bob''s");
        assert_eq!(
            ps_quote("a\u{2019}b\u{2018}"),
            "a\u{2019}\u{2019}b\u{2018}\u{2018}"
        );
    }

    #[test]
    fn installs_only_into_an_empty_or_katna_pdf_folder() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::from_lookup(Scope::User, windows)
            .unwrap()
            .in_folder(dir.path());
        // A new folder, then an empty one.
        layout.check_folder().unwrap();
        std::fs::create_dir(&layout.programs).unwrap();
        layout.check_folder().unwrap();
        // What an uninstall left: only Setup.
        std::fs::write(layout.programs.join("Katna-PDF-Setup.exe"), b"").unwrap();
        layout.check_folder().unwrap();
        // Someone else's files.
        std::fs::write(layout.programs.join("other.exe"), b"").unwrap();
        assert!(layout.check_folder().is_err());
        // Katna PDF's own folder, for an update.
        std::fs::write(layout.programs.join(APP_EXE), b"").unwrap();
        layout.check_folder().unwrap();
    }
}
