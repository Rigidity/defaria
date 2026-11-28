//! Who may run commands: the operators, listed by id in the world's
//! `ops.json5` (see `docs/world-format.md`), and the player hosting a
//! world in the game's own process ([`Host`]). The console always may.

use std::{collections::BTreeSet, io, path::Path};

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use defaria_core::{identity::PlayerId, save::write_atomically};

/// The operator file's name, in the world's folder.
pub const OPS_FILE: &str = "ops.json5";
/// The operator file format this build writes.
pub const OPS_FORMAT: u32 = 1;

/// The player whose game this server runs in (single player): always
/// allowed to run commands. A dedicated server has none.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host(pub PlayerId);

/// The players allowed to run commands, as `ops.json5` lists them (or as
/// a test sets them, for a world in memory).
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct Operators(pub BTreeSet<PlayerId>);

/// `ops.json5`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpsFile {
    format: u32,
    /// Players' ids, as 64 hex digits.
    operators: Vec<String>,
}

/// Reads the operators of the world in `folder`: none if there's no file.
/// A file that can't be read, or is newer than this build, is an error
/// (and nobody's allowed meanwhile).
pub fn read(folder: &Path) -> Result<Operators, String> {
    let path = folder.join(OPS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(default()),
        Err(error) => return Err(format!("can't read {}: {error}", path.display())),
    };
    let file: OpsFile =
        json5::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    if file.format > OPS_FORMAT {
        return Err(format!(
            "{} is format {}, newer than this build's {OPS_FORMAT}",
            path.display(),
            file.format
        ));
    }
    let ids = file
        .operators
        .iter()
        .map(|id| id.parse::<PlayerId>())
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(Operators(ids))
}

/// Writes `operators` as the operators of the world in `folder`.
pub fn write(folder: &Path, operators: &Operators) -> Result<(), String> {
    let path = folder.join(OPS_FILE);
    let file = OpsFile {
        format: OPS_FORMAT,
        operators: operators.0.iter().map(PlayerId::to_string).collect(),
    };
    let text = json5::to_string(&file).map_err(|error| format!("{}: {error}", path.display()))?;
    let text = format!(
        "// The players allowed to run server commands; see docs/world-format.md.\n{text}\n"
    );
    std::fs::create_dir_all(folder)
        .and_then(|()| write_atomically(&path, text.as_bytes()))
        .map_err(|error| format!("can't write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::testing::temp_folder;

    #[test]
    fn operators_are_written_and_read_back() {
        let folder = temp_folder("ops");
        assert_eq!(read(&folder), Ok(Operators::default()));
        let ops = Operators([PlayerId([1; 32]), PlayerId([0xab; 32])].into());
        write(&folder, &ops).unwrap();
        assert_eq!(read(&folder), Ok(ops));
        let text = std::fs::read_to_string(folder.join(OPS_FILE)).unwrap();
        assert!(text.contains(&"ab".repeat(32)), "{text}");
    }

    #[test]
    fn a_bad_or_newer_file_is_an_error() {
        let folder = temp_folder("bad-ops");
        let path = folder.join(OPS_FILE);
        std::fs::create_dir_all(&folder).unwrap();
        for bad in [
            "{ format: 1, operators: [\"nope\"] }",
            "{ format: 2, operators: [] }",
            "{ format: 1 }",
            "not json",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(read(&folder).is_err(), "{bad}");
        }
        std::fs::write(
            &path,
            format!("{{ format: 1, operators: [\"{}\"] }}", "0F".repeat(32)),
        )
        .unwrap();
        assert_eq!(read(&folder), Ok(Operators([PlayerId([0x0f; 32])].into())));
    }
}
