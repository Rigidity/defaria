//! Who this install is, and which servers it knows. Kept in the game's
//! data folder ([`DataFolder`]; see `docs/world-format.md`):
//!
//! - `identity.pem`: the install's key pair (ECDSA P-256, PKCS#8 PEM),
//!   made the first time it's needed and kept for good. It's presented to
//!   servers as a self-signed TLS client certificate, made afresh from the
//!   key each time; the player's id is the hash of its public key
//!   ([`PlayerId`]), so the same install is the same player everywhere,
//!   single player included.
//! - `profile.json5`: the player's name, shown to others (only shown: the
//!   id is who they are).
//! - `servers.json5`: each server this install has connected to and its
//!   key's fingerprint, trusted the first time and checked every time
//!   after, as SSH does ([`KnownServers`]).
//!
//! A server's own key (`server.pem`) is the same kind of file, in the
//! folder of the world it serves, so it stays the same server across
//! restarts.
//!
//! Without a data folder (tests, and anything given none), the identity is
//! made in memory and lasts as long as the app.

use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use bevy::prelude::*;
use defaria_core::identity::{DEFAULT_NAME, PlayerId, clean_name, hex, parse_hex};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

/// The install's key pair, in its data folder.
pub const KEY_FILE: &str = "identity.pem";
/// The player's name, in the data folder.
pub const PROFILE_FILE: &str = "profile.json5";
/// The servers the install knows, in the data folder.
pub const SERVERS_FILE: &str = "servers.json5";
/// A server's key pair, in the folder of the world it serves.
pub const SERVER_KEY_FILE: &str = "server.pem";
/// The `profile.json5` format this build writes.
pub const PROFILE_FORMAT: u32 = 1;
/// The `servers.json5` format this build writes.
pub const SERVERS_FORMAT: u32 = 1;
/// The name certificates are made for (a server's is asked for by it).
pub const CERT_NAME: &str = "defaria";

/// The game's data folder: where the install's identity, profile and
/// known servers are kept (and, for the game itself, its worlds). Without
/// it, they're kept in memory.
#[derive(Resource, Debug, Clone, PartialEq, Eq)]
pub struct DataFolder(pub PathBuf);

/// A key pair: an install's (a player) or a server's.
#[derive(Clone)]
pub struct Identity {
    /// The private key, PKCS#8 DER.
    key: Arc<Vec<u8>>,
    /// The hash of the public key.
    pub id: PlayerId,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity({})", self.id)
    }
}

impl Identity {
    /// A new key pair, kept nowhere.
    pub fn generate() -> Self {
        let key = rcgen::KeyPair::generate().expect("ring makes P-256 keys");
        Self::from_key(&key).expect("a key just made makes a certificate")
    }

    fn from_key(key: &rcgen::KeyPair) -> Result<Self, String> {
        let der = key.serialize_der();
        let mut identity = Self {
            key: Arc::new(der),
            id: PlayerId([0; 32]),
        };
        // Hashed from the certificate as the other side reads it, so both
        // agree byte for byte.
        let (cert, _) = identity.certificate()?;
        identity.id = PlayerId(key_hash(&cert)?);
        Ok(identity)
    }

    /// The key pair in `path`, or a new one written there if there's no
    /// file yet. A file that can't be read is an error, and left alone: it
    /// may still be someone's only key. Two games starting at once agree:
    /// the new key is only ever put in place if there's none, and the one
    /// that loses reads the winner's.
    pub fn load_or_create(path: &Path) -> Result<Self, String> {
        let read = |pem: &str| {
            let key = rcgen::KeyPair::from_pem(pem)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Self::from_key(&key).map_err(|error| format!("{}: {error}", path.display()))
        };
        match std::fs::read_to_string(path) {
            Ok(pem) => read(&pem),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let key = rcgen::KeyPair::generate().map_err(|error| error.to_string())?;
                if let Some(folder) = path.parent() {
                    std::fs::create_dir_all(folder)
                        .map_err(|error| format!("can't create {}: {error}", folder.display()))?;
                }
                let made = write_new_secret(path, key.serialize_pem().as_bytes())
                    .map_err(|error| format!("can't write {}: {error}", path.display()))?;
                if !made {
                    // Another copy of the game made one first: theirs is
                    // the install's.
                    let pem = std::fs::read_to_string(path)
                        .map_err(|error| format!("can't read {}: {error}", path.display()))?;
                    return read(&pem);
                }
                info!("made a new key in {}", path.display());
                Self::from_key(&key)
            }
            Err(error) => Err(format!("can't read {}: {error}", path.display())),
        }
    }

    /// A self-signed certificate for this key, and the key, for TLS.
    pub fn certificate(&self) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), String> {
        let der = PrivatePkcs8KeyDer::from(self.key.as_slice());
        let key = rcgen::KeyPair::try_from(&der).map_err(|error| error.to_string())?;
        let cert = rcgen::CertificateParams::new(vec![CERT_NAME.to_string()])
            .and_then(|params| params.self_signed(&key))
            .map_err(|error| error.to_string())?;
        Ok((
            cert.der().clone(),
            PrivatePkcs8KeyDer::from(self.key.as_ref().clone()).into(),
        ))
    }
}

/// A temporary file's name next to `path`, unique to this process and
/// call, so two writers never share one.
fn temp_beside(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let mut name = path.as_os_str().to_owned();
    name.push(format!(
        ".{}.{}.{nanos}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(name)
}

/// Writes `data` to a new temporary file next to `path`, readable only by
/// its owner from the start (on unix; on Windows the data folder's own
/// per-user permissions keep it), and synced to disk.
fn write_temp(path: &Path, data: &[u8], secret: bool) -> io::Result<PathBuf> {
    use std::io::Write;
    let temp = temp_beside(path);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if secret {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = secret;
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(data)?;
        file.sync_all()
    });
    match written {
        Ok(()) => Ok(temp),
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(error)
        }
    }
}

/// Puts a private key at `path`, readable only by its owner, if there's
/// nothing there yet: whether it did. Never replaces a file, and never
/// leaves `path` half written.
fn write_new_secret(path: &Path, data: &[u8]) -> io::Result<bool> {
    let temp = write_temp(path, data, true)?;
    // A hard link fails if `path` exists, where a rename would replace it.
    let linked = std::fs::hard_link(&temp, path);
    let _ = std::fs::remove_file(&temp);
    match linked {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(error),
    }
}

/// Writes `data` to `path` whole, through a temporary file of its own (so
/// two writers don't trip over each other's), replacing what's there.
fn write_replacing(path: &Path, data: &[u8]) -> io::Result<()> {
    let temp = write_temp(path, data, false)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// The SHA-256 hash of the public key `cert` holds (its DER
/// `SubjectPublicKeyInfo`): a player's id, or a server's fingerprint.
pub fn key_hash(cert: &CertificateDer<'_>) -> Result<[u8; 32], String> {
    let parsed = webpki::EndEntityCert::try_from(cert).map_err(|error| error.to_string())?;
    let spki = parsed.subject_public_key_info();
    let digest = ring::digest::digest(&ring::digest::SHA256, spki.as_ref());
    Ok(digest.as_ref().try_into().expect("SHA-256 is 32 bytes"))
}

/// This install's identity and name, as the client plays with them.
#[derive(Resource, Debug, Clone)]
pub struct LocalIdentity {
    pub identity: Identity,
    pub name: String,
    /// Why the install's key couldn't be read, if it couldn't: the
    /// identity is a stand-in, fine for single player, but not for joining
    /// a server, where it would be a stray player.
    pub damaged: Option<String>,
}

impl LocalIdentity {
    /// The install's identity and name from `folder`, made there if
    /// they're not yet; with no folder, or one whose key can't be read
    /// (reported, and left alone), a new identity in memory.
    pub fn load(folder: Option<&Path>) -> Self {
        let Some(folder) = folder else {
            return Self {
                identity: Identity::generate(),
                name: DEFAULT_NAME.to_string(),
                damaged: None,
            };
        };
        let (identity, damaged) = match Identity::load_or_create(&folder.join(KEY_FILE)) {
            Ok(identity) => (identity, None),
            Err(error) => {
                error!("{error}; playing as someone new this time, and joining no servers");
                (Identity::generate(), Some(error))
            }
        };
        let name = load_name(folder).unwrap_or_else(|error| {
            error!("{error}");
            DEFAULT_NAME.to_string()
        });
        Self {
            identity,
            name,
            damaged,
        }
    }
}

/// The client's [`LocalIdentity`], loaded from its [`DataFolder`] the
/// first time it's asked for.
pub fn local_identity(world: &mut World) -> LocalIdentity {
    if let Some(local) = world.get_resource::<LocalIdentity>() {
        return local.clone();
    }
    let folder = world.get_resource::<DataFolder>().map(|f| f.0.clone());
    let local = LocalIdentity::load(folder.as_deref());
    info!("playing as {} ({})", local.name, local.identity.id);
    world.insert_resource(local.clone());
    local
}

/// `profile.json5`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub format: u32,
    /// The name others see.
    pub name: String,
}

/// The player's name from `folder`'s `profile.json5`, written with the
/// default name if there isn't one yet.
pub fn load_name(folder: &Path) -> Result<String, String> {
    let path = folder.join(PROFILE_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let profile: Profile =
                json5::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            if profile.format > PROFILE_FORMAT {
                return Err(format!(
                    "{} is profile format {}, newer than this build's {PROFILE_FORMAT}",
                    path.display(),
                    profile.format
                ));
            }
            Ok(clean_name(&profile.name))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let profile = Profile {
                format: PROFILE_FORMAT,
                name: DEFAULT_NAME.to_string(),
            };
            let text = json5::to_string(&profile).map_err(|error| error.to_string())?;
            let text = format!(
                "// Who you are in Defaria: the name others see. See docs/world-format.md.\n\
                 {text}\n"
            );
            write_replacing(&path, text.as_bytes())
                .map_err(|error| format!("can't write {}: {error}", path.display()))?;
            Ok(profile.name)
        }
        Err(error) => Err(format!("can't read {}: {error}", path.display())),
    }
}

/// A server key's fingerprint: the SHA-256 hash of its public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// `servers.json5`: the servers this install has played on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServersFile {
    pub format: u32,
    pub servers: Vec<KnownServer>,
}

/// A server this install has played on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownServer {
    /// Its address, as it was given (`host:port`).
    pub address: String,
    /// Its key's fingerprint, 64 hex digits.
    pub fingerprint: String,
}

/// The servers this install knows, and the file they're kept in (none:
/// in memory, forgotten when the game stops).
#[derive(Debug, Clone, Default)]
pub struct KnownServers {
    path: Option<PathBuf>,
    file: ServersFile,
}

impl KnownServers {
    /// The servers in `folder`'s `servers.json5` (none yet if it has
    /// none). One that can't be read is an error: connecting without it
    /// would trust any server.
    pub fn load(folder: Option<&Path>) -> Result<Self, String> {
        let Some(folder) = folder else {
            return Ok(Self::default());
        };
        let path = folder.join(SERVERS_FILE);
        Ok(Self {
            file: read_servers(&path)?,
            path: Some(path),
        })
    }

    /// The fingerprint remembered for the server at `address`.
    pub fn fingerprint(&self, address: &str) -> Option<Fingerprint> {
        let known = self.file.servers.iter().find(|s| s.address == address)?;
        parse_hex(&known.fingerprint).map(Fingerprint)
    }

    /// Remembers `fingerprint` for the server at `address`, if nothing's
    /// remembered for it yet, and writes the file. The file is read again
    /// first, so what another copy of the game added since this one read it
    /// is kept; if that other copy remembered a different key for the same
    /// address meanwhile, theirs stays and this is an error.
    pub fn remember(&mut self, address: &str, fingerprint: Fingerprint) -> Result<(), String> {
        if let Some(path) = &self.path {
            self.file = read_servers(path)?;
        }
        match self.fingerprint(address) {
            Some(known) if known == fingerprint => return Ok(()),
            Some(known) => {
                return Err(format!(
                    "another copy of the game remembered a different key for {address} \
                     ({known}) while this one connected"
                ));
            }
            None => {}
        }
        self.file.servers.push(KnownServer {
            address: address.to_string(),
            fingerprint: fingerprint.to_string(),
        });
        self.file.format = SERVERS_FORMAT;
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = json5::to_string(&self.file).map_err(|error| error.to_string())?;
        let text = format!(
            "// The servers this game has played on, and their keys' fingerprints.\n\
             // A server whose key changes is refused; see docs/world-format.md.\n{text}\n"
        );
        write_replacing(path, text.as_bytes())
            .map_err(|error| format!("can't write {}: {error}", path.display()))
    }

    /// The file they're kept in, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
}

/// `servers.json5` at `path`: empty if there's none.
fn read_servers(path: &Path) -> Result<ServersFile, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let file: ServersFile =
                json5::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            if file.format > SERVERS_FORMAT {
                return Err(format!(
                    "{} is format {}, newer than this build's {SERVERS_FORMAT}",
                    path.display(),
                    file.format
                ));
            }
            Ok(file)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ServersFile::default()),
        Err(error) => Err(format!("can't read {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use defaria_core::testing::temp_folder;

    /// Two installs have different ids; the same install keeps its id,
    /// read back from its key file.
    #[test]
    fn each_install_has_its_own_lasting_id() {
        let first = temp_folder("identity-first");
        let second = temp_folder("identity-second");
        let a = Identity::load_or_create(&first.join(KEY_FILE)).unwrap();
        let b = Identity::load_or_create(&second.join(KEY_FILE)).unwrap();
        assert_ne!(a.id, b.id);
        let again = Identity::load_or_create(&first.join(KEY_FILE)).unwrap();
        assert_eq!(again.id, a.id);
        // The certificate is made afresh, but the key, and so the id, is
        // the same.
        let (cert, _) = again.certificate().unwrap();
        assert_eq!(PlayerId(key_hash(&cert).unwrap()), a.id);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(first.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    /// A key file that can't be read is an error, and left as it is.
    #[test]
    fn a_damaged_key_is_left_alone() {
        let folder = temp_folder("identity-damaged");
        let path = folder.join(KEY_FILE);
        std::fs::write(&path, "not a key").unwrap();
        assert!(Identity::load_or_create(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a key");
        // The game plays on as someone new, without writing over it.
        let local = LocalIdentity::load(Some(&folder));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not a key");
        assert_eq!(local.name, DEFAULT_NAME);
        assert!(local.damaged.is_some(), "and says so, to join no servers");
        assert!(LocalIdentity::load(None).damaged.is_none());
    }

    /// Copies of the game making the install's key at once all end up
    /// with the one key on disk, and leave no temporary files.
    #[test]
    fn keys_made_at_once_agree() {
        let folder = temp_folder("identity-race");
        let path = folder.join(KEY_FILE);
        let ids: Vec<PlayerId> = std::thread::scope(|scope| {
            let made: Vec<_> = (0..8)
                .map(|_| scope.spawn(|| Identity::load_or_create(&path).unwrap().id))
                .collect();
            made.into_iter().map(|made| made.join().unwrap()).collect()
        });
        let on_disk = Identity::load_or_create(&path).unwrap().id;
        assert!(ids.iter().all(|id| *id == on_disk), "{ids:?}");
        let files: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "{files:?}");
    }

    #[test]
    fn the_name_comes_from_the_profile() {
        let folder = temp_folder("profile");
        assert_eq!(load_name(&folder).unwrap(), DEFAULT_NAME);
        assert!(folder.join(PROFILE_FILE).exists());
        std::fs::write(
            folder.join(PROFILE_FILE),
            "{ format: 1, name: '  Ada Lovelace  ' }",
        )
        .unwrap();
        assert_eq!(load_name(&folder).unwrap(), "Ada Lovelace");
        std::fs::write(folder.join(PROFILE_FILE), "{ format: 9, name: 'x' }").unwrap();
        assert!(load_name(&folder).unwrap_err().contains("newer"));
    }

    #[test]
    fn known_servers_are_remembered() {
        let folder = temp_folder("known-servers");
        let mut known = KnownServers::load(Some(&folder)).unwrap();
        assert_eq!(known.fingerprint("example.org:25575"), None);
        let print = Fingerprint([7; 32]);
        known.remember("example.org:25575", print).unwrap();
        known.remember("localhost:1", Fingerprint([1; 32])).unwrap();
        let again = KnownServers::load(Some(&folder)).unwrap();
        assert_eq!(again.fingerprint("example.org:25575"), Some(print));
        assert_eq!(again.fingerprint("localhost:1"), Some(Fingerprint([1; 32])));
        assert_eq!(again.file.servers.len(), 2);
        std::fs::write(folder.join(SERVERS_FILE), "{ format: 9, servers: [] }").unwrap();
        assert!(KnownServers::load(Some(&folder)).is_err());
    }

    /// Two copies of the game remembering servers at once keep both; the
    /// first to remember an address keeps its key.
    #[test]
    fn known_servers_from_two_copies_are_merged() {
        let folder = temp_folder("known-servers-merge");
        let mut first = KnownServers::load(Some(&folder)).unwrap();
        let mut second = KnownServers::load(Some(&folder)).unwrap();
        first.remember("a:1", Fingerprint([1; 32])).unwrap();
        second.remember("b:2", Fingerprint([2; 32])).unwrap();
        let both = KnownServers::load(Some(&folder)).unwrap();
        assert_eq!(both.fingerprint("a:1"), Some(Fingerprint([1; 32])));
        assert_eq!(both.fingerprint("b:2"), Some(Fingerprint([2; 32])));
        let error = first.remember("b:2", Fingerprint([3; 32])).unwrap_err();
        assert!(error.contains("different key"), "{error}");
        let after = KnownServers::load(Some(&folder)).unwrap();
        assert_eq!(after.fingerprint("b:2"), Some(Fingerprint([2; 32])));
        let files: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "no temporary files left: {files:?}");
    }
}
