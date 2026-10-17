use std::path::{Path, PathBuf};

use omega_store::{atomic_write, read_app_state, write_app_state, Core, Platform, StateRing};

pub trait Snapshot {
    fn state(&self) -> Option<Vec<u8>>;
    fn save_ram(&self) -> Option<Vec<u8>>;
    fn thumb(&self) -> Option<Vec<u8>>;
    fn load(&self, state: Vec<u8>);

    /// Whether a game is running whose progress a save protects. A core still loading, or
    /// one that failed to start, has nothing new to lose.
    fn running(&self) -> bool {
        true
    }

    fn resume_trusted(&self) -> bool {
        true
    }

    fn save_ram_trusted(&self) -> bool {
        true
    }
}

pub fn flush(
    root: &Path,
    platform: Platform,
    core: Core,
    stem: &str,
    state: Option<&[u8]>,
    sav: Option<&[u8]>,
) -> std::io::Result<()> {
    if let Some(state) = state {
        StateRing::new(root, platform, core, stem).write_resume(state)?;
    }
    if let Some(sav) = sav {
        write_sav(root, platform, stem, sav)?;
    }
    Ok(())
}

/// Where the game whose core is opening is named, as `<system>/<stem>`, until it has run a
/// moment. A boot that finds it names a game whose start took omega-zero down, and leaves that
/// game on the shelf: a core that aborts as it opens a game would otherwise have every boot open
/// it again, and the console would never get past its boot screen.
const STARTING: &str = "Config/.starting";

fn starting_key(platform: Platform, stem: &str) -> String {
    format!("{}/{stem}", platform.dir_name())
}

/// Notes that `stem`'s core is opening, on the card before it opens.
pub fn mark_starting(root: &Path, platform: Platform, stem: &str) {
    let key = starting_key(platform, stem);
    if let Err(e) = atomic_write(&root.join(STARTING), key.as_bytes()) {
        eprintln!("omega-zero: could not note {key} starting: {e}");
    }
}

pub fn clear_starting(root: &Path) {
    let _ = std::fs::remove_file(root.join(STARTING));
}

/// The game noted as starting when omega-zero last stopped, the note taken off the card.
pub fn take_starting(root: &Path) -> Option<String> {
    let key = std::fs::read_to_string(root.join(STARTING)).ok()?;
    clear_starting(root);
    Some(key)
}

/// The system and stem a note of a game starting names, as `mark_starting` wrote it.
pub fn starting_game(key: &str) -> Option<(Platform, String)> {
    let (dir, stem) = key.trim_end_matches(['\r', '\n']).split_once('/')?;
    let platform = Platform::ALL.into_iter().find(|p| p.dir_name() == dir)?;
    (!stem.is_empty()).then(|| (platform, stem.to_string()))
}

pub fn eject(
    root: &Path,
    platform: Platform,
    core: Core,
    stem: &str,
    state: Option<&[u8]>,
    sav: Option<&[u8]>,
) -> std::io::Result<()> {
    flush(root, platform, core, stem, state, sav)?;
    let mut state = read_app_state(root);
    state.cart = None;
    state.cart_platform = None;
    write_app_state(root, &state)
}

pub fn write_sav(root: &Path, platform: Platform, stem: &str, sav: &[u8]) -> std::io::Result<bool> {
    let path = sav_path(root, platform, stem);
    if let Some(old) = read_sav(root, platform, stem) {
        if old == sav {
            return Ok(false);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    atomic_write(&path, sav)?;
    Ok(true)
}

pub fn read_sav(root: &Path, platform: Platform, stem: &str) -> Option<Vec<u8>> {
    std::fs::read(sav_path(root, platform, stem))
        .or_else(|_| {
            std::fs::read(
                crate::root::saves_dir(root)
                    .join(platform.dir_name())
                    .join(format!("{stem}.srm")),
            )
        })
        .ok()
}

pub fn read_resume(root: &Path, platform: Platform, core: Core, stem: &str) -> Option<Vec<u8>> {
    StateRing::new(root, platform, core, stem)
        .read_resume()
        .ok()
        .flatten()
}

fn sav_path(root: &Path, platform: Platform, stem: &str) -> PathBuf {
    crate::root::saves_dir(root)
        .join(platform.dir_name())
        .join(format!("{stem}.sav"))
}

/// Where a game on several discs keeps the disc it was last swapped to, beside its save: the
/// disc's number counting from 0 on one line and the file the core reads it from on the next.
fn disc_path(root: &Path, platform: Platform, stem: &str) -> PathBuf {
    crate::root::saves_dir(root)
        .join(platform.dir_name())
        .join(format!("{stem}.disc"))
}

/// Keeps disc `index`, read from `path`, as the one the game is on.
pub fn write_disc(
    root: &Path,
    platform: Platform,
    stem: &str,
    index: usize,
    path: &str,
) -> std::io::Result<()> {
    let at = disc_path(root, platform, stem);
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir)?;
    }
    atomic_write(&at, format!("{index}\n{path}\n").as_bytes())
}

/// The disc a game was last swapped to and the file it is read from, if it ever was.
pub fn read_disc(root: &Path, platform: Platform, stem: &str) -> Option<(usize, String)> {
    let text = std::fs::read_to_string(disc_path(root, platform, stem)).ok()?;
    let mut lines = text.lines();
    let index = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.to_string();
    Some((index, path))
}
