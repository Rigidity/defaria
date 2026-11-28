//! The saved servers: `servers.json5` in the game's data folder
//! ([`DataFolder`](crate::save::DataFolder)), the list the Multiplayer
//! screen shows, in its order, and what each server's key is known to be.
//!
//! Each server has a name (only shown; the address when it has none), an
//! address (`host:port`, the port [`DEFAULT_PORT`] when not given, which
//! is also what it's known by) and, once the game has connected to it,
//! its key's [`Fingerprint`]: trusted the first time, and checked every
//! time after, as SSH does, so a server at that address with another key
//! is refused until the player trusts its new key, which then replaces
//! the old ([`ServerList::replace_key`]).
//! Connecting to an address not on the list (`--connect`) adds it.
//!
//! Every change reads the file again first and writes it whole through a
//! temporary file of its own, so what another copy of the game changed
//! meanwhile is kept. A file that can't be trusted to say which key a
//! server has (a fingerprint that isn't one, two entries for one address)
//! is unreadable, as a file that doesn't parse is: connecting then refuses
//! rather than trusting any key. The format is in `docs/world-format.md`.

use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    identity::{hex, parse_hex},
    save::write_replacing,
};

/// The saved servers, in the data folder.
pub const SERVERS_FILE: &str = "servers.json5";
/// The `servers.json5` format this build writes. 2 gave servers names (a
/// format 1 file, of addresses and fingerprints alone, reads as a list of
/// unnamed servers).
pub const SERVERS_FORMAT: u32 = 2;
/// The port a server listens on unless told otherwise, and the one an
/// address without a port means.
pub const DEFAULT_PORT: u16 = 25_575;
/// The longest a server's name is, in characters.
pub const MAX_NAME: usize = 32;
/// The longest an address is, in bytes: a host name of 253, a colon and
/// a port, and an IPv6 address's brackets.
pub const MAX_ADDRESS: usize = 263;

/// A server key's fingerprint: the SHA-256 hash of its public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

/// `servers.json5`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServersFile {
    pub format: u32,
    /// In the order they're listed.
    pub servers: Vec<SavedServer>,
}

/// A saved server.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedServer {
    /// What it's called on the list; none shows its address.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Where it is, `host:port` ([`normalize_address`]): also what it's
    /// known by, so no two servers on the list share one.
    pub address: String,
    /// Its key's fingerprint, 64 hex digits, once the game has connected
    /// to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

impl SavedServer {
    /// What the list calls it: its name, or its address without one.
    pub fn shown_name(&self) -> &str {
        if self.name.is_empty() {
            &self.address
        } else {
            &self.name
        }
    }
}

/// The saved servers, and the file they're kept in (none: in memory,
/// forgotten when the game stops).
#[derive(Debug, Clone, Default)]
pub struct ServerList {
    path: Option<PathBuf>,
    file: ServersFile,
}

impl ServerList {
    /// The servers in `folder`'s `servers.json5` (none yet if it has
    /// none). One that can't be read is an error: connecting without it
    /// would trust any server, and changing it would lose what's there.
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

    /// The servers, in their order.
    pub fn servers(&self) -> &[SavedServer] {
        &self.file.servers
    }

    /// The server at `address`.
    pub fn find(&self, address: &str) -> Option<&SavedServer> {
        self.file.servers.iter().find(|s| s.address == address)
    }

    /// The fingerprint remembered for the server at `address`.
    pub fn fingerprint(&self, address: &str) -> Option<Fingerprint> {
        let known = self.find(address)?.fingerprint.as_deref()?;
        parse_hex(known).map(Fingerprint)
    }

    /// Adds a server called `name` at `address`, at the end of the list,
    /// and returns its address as it's kept.
    pub fn add(&mut self, name: &str, address: &str) -> Result<String, String> {
        let name = clean_server_name(name);
        let address = normalize_address(address)?;
        let kept = address.clone();
        self.change(move |servers| {
            if let Some(there) = servers.iter().find(|s| s.address == address) {
                return Err(already_listed(there));
            }
            servers.push(SavedServer {
                name,
                address,
                fingerprint: None,
            });
            Ok(())
        })?;
        Ok(kept)
    }

    /// Renames the server at `address` to `name` and moves it to
    /// `new_address`, and returns that as it's kept. A server given another
    /// address is another server: its key is forgotten, to be trusted
    /// afresh.
    pub fn edit(&mut self, address: &str, name: &str, new_address: &str) -> Result<String, String> {
        let name = clean_server_name(name);
        let new_address = normalize_address(new_address)?;
        let kept = new_address.clone();
        self.change(move |servers| {
            if new_address != address
                && let Some(there) = servers.iter().find(|s| s.address == new_address)
            {
                return Err(already_listed(there));
            }
            let server = servers
                .iter_mut()
                .find(|s| s.address == address)
                .ok_or_else(|| not_listed(address))?;
            server.name = name;
            if server.address != new_address {
                server.address = new_address;
                server.fingerprint = None;
            }
            Ok(())
        })?;
        Ok(kept)
    }

    /// Takes the server at `address` off the list, forgetting its key.
    pub fn remove(&mut self, address: &str) -> Result<(), String> {
        self.change(|servers| {
            let before = servers.len();
            servers.retain(|s| s.address != address);
            if servers.len() == before {
                return Err(not_listed(address));
            }
            Ok(())
        })
    }

    /// Replaces the key remembered for the server at `address`, `old`,
    /// with `new` (the player trusted the new one, its owner having changed
    /// it, say), once a connection has shown it holds `new`. If the key
    /// saved isn't `old` any more (another copy of the game changed it),
    /// nothing changes and this is an error.
    pub fn replace_key(
        &mut self,
        address: &str,
        old: Fingerprint,
        new: Fingerprint,
    ) -> Result<(), String> {
        self.change(|servers| {
            let server = servers
                .iter_mut()
                .find(|s| s.address == address)
                .ok_or_else(|| not_listed(address))?;
            let saved = server.fingerprint.as_deref().and_then(parse_hex);
            if saved != Some(old.0) {
                return Err(format!(
                    "the key saved for {address} changed while connecting, so it wasn't replaced"
                ));
            }
            server.fingerprint = Some(new.to_string());
            Ok(())
        })
    }

    /// Remembers `fingerprint` for the server at `address`, if nothing's
    /// remembered for it yet, adding it to the end of the list (unnamed)
    /// if it isn't on it. If another copy of the game remembered a
    /// different key for it meanwhile, theirs stays and this is an error.
    pub fn remember(&mut self, address: &str, fingerprint: Fingerprint) -> Result<(), String> {
        let print = fingerprint.to_string();
        self.change(|servers| {
            match servers.iter_mut().find(|s| s.address == address) {
                Some(server) => match &server.fingerprint {
                    Some(known) if parse_hex(known) == Some(fingerprint.0) => {}
                    Some(known) => {
                        return Err(format!(
                            "another copy of the game remembered a different key for \
                             {address} ({known}) while this one connected"
                        ));
                    }
                    None => server.fingerprint = Some(print),
                },
                None => servers.push(SavedServer {
                    name: String::new(),
                    address: address.to_string(),
                    fingerprint: Some(print),
                }),
            }
            Ok(())
        })
    }

    /// The file they're kept in, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Reads the file again, changes its servers with `change`, and writes
    /// it, unless `change` fails.
    fn change(
        &mut self,
        change: impl FnOnce(&mut Vec<SavedServer>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut file = match &self.path {
            Some(path) => read_servers(path)?,
            None => self.file.clone(),
        };
        change(&mut file.servers)?;
        file.format = SERVERS_FORMAT;
        if let Some(path) = &self.path {
            write_replacing(path, to_json5(&file).as_bytes())
                .map_err(|error| format!("can't write {}: {error}", path.display()))?;
        }
        self.file = file;
        Ok(())
    }
}

fn already_listed(server: &SavedServer) -> String {
    format!(
        "{} is already on the list, as {}",
        server.address,
        server.shown_name()
    )
}

fn not_listed(address: &str) -> String {
    format!("{address} isn't on the list (another copy of the game took it off?)")
}

/// `file` as it's written: a server a line, in order.
fn to_json5(file: &ServersFile) -> String {
    let mut text = String::from(
        "// The servers on the Multiplayer screen, in order, and their keys'\n\
         // fingerprints. A server whose key changes is refused; see\n\
         // docs/world-format.md.\n",
    );
    text.push_str(&format!("{{\n  format: {},\n  servers: [\n", file.format));
    let quoted = |text: &str| json5::to_string(&text).expect("a string is plain data");
    for server in &file.servers {
        let mut fields = Vec::new();
        if !server.name.is_empty() {
            fields.push(format!("name: {}", quoted(&server.name)));
        }
        fields.push(format!("address: {}", quoted(&server.address)));
        if let Some(fingerprint) = &server.fingerprint {
            fields.push(format!("fingerprint: {}", quoted(fingerprint)));
        }
        text.push_str(&format!("    {{ {} }},\n", fields.join(", ")));
    }
    text.push_str("  ],\n}\n");
    text
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
            checked(file).map_err(|error| format!("{}: {error}", path.display()))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ServersFile::default()),
        Err(error) => Err(format!("can't read {}: {error}", path.display())),
    }
}

/// `file` as it's used: every address normalized, and none twice, and
/// every fingerprint 64 hex digits. Anything else is an error, not
/// skipped: a server whose key can't be read, or that two entries
/// disagree about, would otherwise be trusted with any key.
fn checked(mut file: ServersFile) -> Result<ServersFile, String> {
    let mut seen = std::collections::HashSet::new();
    for server in &mut file.servers {
        let address = normalize_address(&server.address)
            .map_err(|error| format!("`{}` isn't an address: {error}", server.address))?;
        if !seen.insert(address.clone()) {
            return Err(format!(
                "{address} is on the list twice; take one off so it's clear which key it has"
            ));
        }
        if let Some(print) = &server.fingerprint
            && parse_hex(print).is_none()
        {
            return Err(format!(
                "the fingerprint saved for {address} isn't 64 hex digits"
            ));
        }
        server.address = address;
    }
    Ok(file)
}

/// A server's name as it's kept: trimmed, without control or format
/// characters, at most [`MAX_NAME`] characters. Empty is fine (the list
/// shows the address).
pub fn clean_server_name(name: &str) -> String {
    let kept: String = name
        .chars()
        .filter(|c| !c.is_control() && !crate::identity::is_format(*c))
        .collect();
    let kept: String = kept.trim().chars().take(MAX_NAME).collect();
    kept.trim_end().to_string()
}

/// An address as it's kept and connected to: trimmed, its host in
/// lowercase, with the [`DEFAULT_PORT`] if it has no port (`Example.org`
/// is `example.org:25575`; a bare IPv6 address goes in brackets). A host
/// is a name or IPv4 address (ASCII letters, digits, `-`, `_` and `.`) or
/// an IPv6 address in brackets (hex digits, `:` and `.`).
pub fn normalize_address(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(
            "give the server's address: a host, and a port after a colon \
                    unless it's the usual one"
                .into(),
        );
    }
    if text.len() > MAX_ADDRESS {
        return Err("that address is too long".into());
    }
    if text.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("an address has no spaces in it".into());
    }
    let (host, port) = if let Some(rest) = text.strip_prefix('[') {
        let (inside, after) = rest
            .split_once(']')
            .ok_or("an IPv6 address in brackets needs its closing bracket")?;
        let port = match after {
            "" => None,
            _ => Some(
                after
                    .strip_prefix(':')
                    .ok_or("after an IPv6 address's brackets comes a colon and the port")?,
            ),
        };
        (format!("[{inside}]"), port)
    } else {
        match text.matches(':').count() {
            0 => (text.to_string(), None),
            1 => {
                let (host, port) = text.split_once(':').expect("one colon");
                (host.to_string(), Some(port))
            }
            // An IPv6 address, without brackets or a port.
            _ => (format!("[{text}]"), None),
        }
    };
    if host.is_empty() || host == "[]" {
        return Err("the address has no host before its port".into());
    }
    let host = host.to_ascii_lowercase();
    let allowed = match host.strip_prefix('[') {
        Some(inside) => inside
            .trim_end_matches(']')
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.'),
        None => host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
    };
    if !allowed {
        return Err(format!(
            "`{host}` isn't a host: a name (letters, digits, `-` and `.`) or an IP address"
        ));
    }
    let port = match port {
        None => DEFAULT_PORT,
        Some(port) => match port.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => return Err(format!("`{port}` isn't a port (a number, 1 to 65535)")),
        },
    };
    Ok(format!("{host}:{port}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::temp_folder;

    #[test]
    fn addresses_get_the_usual_port() {
        let ok = |text: &str| normalize_address(text).unwrap();
        assert_eq!(ok("example.org"), "example.org:25575");
        assert_eq!(ok("  example.org:7000 "), "example.org:7000");
        assert_eq!(ok("127.0.0.1"), "127.0.0.1:25575");
        assert_eq!(ok("::1"), "[::1]:25575");
        assert_eq!(ok("[::1]"), "[::1]:25575");
        assert_eq!(ok("[::1]:9"), "[::1]:9");
        assert_eq!(ok("Example.ORG"), "example.org:25575");
        assert_eq!(ok("[::ABCD]:2"), "[::abcd]:2");
        for bad in [
            "",
            "  ",
            "host:",
            "host:0",
            "host:99999",
            ":80",
            "a b",
            "[::1",
            "[::1]9",
            "a/b",
            "a@b:1",
            "[::g]",
            "héllo",
        ] {
            assert!(normalize_address(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_server_name("  Home \u{202e}base\n"), "Home base");
        assert_eq!(clean_server_name(&"x".repeat(50)).len(), MAX_NAME);
        assert_eq!(clean_server_name("   "), "");
    }

    /// Servers are added, named, moved, given keys and taken off, in
    /// order, and read back the same from the file.
    #[test]
    fn the_list_is_kept_in_order() {
        let folder = temp_folder("server-list");
        let mut list = ServerList::load(Some(&folder)).unwrap();
        assert!(list.servers().is_empty());
        assert_eq!(list.add("Home", "localhost").unwrap(), "localhost:25575");
        list.add("", "example.org:7000").unwrap();
        list.add("Third", "10.0.0.1:1").unwrap();
        let error = list.add("Again", "localhost:25575").unwrap_err();
        assert!(error.contains("already on the list, as Home"), "{error}");

        let again = ServerList::load(Some(&folder)).unwrap();
        let shown: Vec<_> = again
            .servers()
            .iter()
            .map(SavedServer::shown_name)
            .collect();
        assert_eq!(shown, ["Home", "example.org:7000", "Third"]);

        // A key, remembered; renaming keeps it, a new address forgets it.
        let print = Fingerprint([7; 32]);
        list.remember("localhost:25575", print).unwrap();
        list.edit("localhost:25575", "House", "localhost").unwrap();
        assert_eq!(list.fingerprint("localhost:25575"), Some(print));
        let moved = list
            .edit("localhost:25575", "House", "localhost:2")
            .unwrap();
        assert_eq!(moved, "localhost:2");
        assert_eq!(list.fingerprint("localhost:2"), None);
        let error = list.edit("localhost:2", "House", "10.0.0.1:1").unwrap_err();
        assert!(error.contains("already on the list"), "{error}");

        // A new key replaces the old only while the old is the one saved.
        list.remember("localhost:2", print).unwrap();
        let new = Fingerprint([8; 32]);
        let error = list
            .replace_key("localhost:2", Fingerprint([9; 32]), new)
            .unwrap_err();
        assert!(error.contains("changed while connecting"), "{error}");
        assert_eq!(list.fingerprint("localhost:2"), Some(print));
        list.replace_key("localhost:2", print, new).unwrap();
        assert_eq!(list.fingerprint("localhost:2"), Some(new));
        list.remove("example.org:7000").unwrap();
        assert!(list.remove("example.org:7000").is_err());

        let again = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(
            again.servers(),
            [
                SavedServer {
                    name: "House".into(),
                    address: "localhost:2".into(),
                    fingerprint: Some(new.to_string()),
                },
                SavedServer {
                    name: "Third".into(),
                    address: "10.0.0.1:1".into(),
                    fingerprint: None,
                },
            ]
        );
        let text = std::fs::read_to_string(folder.join(SERVERS_FILE)).unwrap();
        assert!(text.contains("format: 2"), "{text}");
        assert!(
            text.contains(r#"    { name: "Third", address: "10.0.0.1:1" },"#),
            "a server a line: {text}"
        );
    }

    /// Connecting to a server remembers its key, adding it to the list if
    /// it isn't there; a newer file is refused.
    #[test]
    fn keys_are_remembered() {
        let folder = temp_folder("server-list-keys");
        let mut list = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(list.fingerprint("example.org:25575"), None);
        let print = Fingerprint([7; 32]);
        list.remember("example.org:25575", print).unwrap();
        list.remember("localhost:1", Fingerprint([1; 32])).unwrap();
        let again = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(again.fingerprint("example.org:25575"), Some(print));
        assert_eq!(again.fingerprint("localhost:1"), Some(Fingerprint([1; 32])));
        assert_eq!(again.servers().len(), 2);
        assert_eq!(again.servers()[0].shown_name(), "example.org:25575");
        std::fs::write(folder.join(SERVERS_FILE), "{ format: 9, servers: [] }").unwrap();
        assert!(ServerList::load(Some(&folder)).is_err());
        std::fs::write(
            folder.join(SERVERS_FILE),
            "{ format: 2, servers: [ { x: 1 } ] }",
        )
        .unwrap();
        assert!(ServerList::load(Some(&folder)).is_err());
    }

    /// A format 1 file, of addresses and fingerprints, is a list of
    /// unnamed servers; the next change writes it as format 2.
    #[test]
    fn a_format_1_file_reads_as_unnamed_servers() {
        let folder = temp_folder("server-list-format-1");
        let print = "07".repeat(32);
        std::fs::write(
            folder.join(SERVERS_FILE),
            format!("{{ format: 1, servers: [ {{ address: 'a:1', fingerprint: '{print}' }} ] }}"),
        )
        .unwrap();
        let mut list = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(list.fingerprint("a:1"), Some(Fingerprint([7; 32])));
        list.edit("a:1", "Named", "a:1").unwrap();
        let again = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(again.servers()[0].name, "Named");
        assert_eq!(again.fingerprint("a:1"), Some(Fingerprint([7; 32])));
    }

    /// Two copies of the game changing the list at once keep both
    /// changes; the first to remember an address's key keeps it.
    #[test]
    fn changes_from_two_copies_are_merged() {
        let folder = temp_folder("server-list-merge");
        let mut first = ServerList::load(Some(&folder)).unwrap();
        let mut second = ServerList::load(Some(&folder)).unwrap();
        first.remember("a:1", Fingerprint([1; 32])).unwrap();
        second.remember("b:2", Fingerprint([2; 32])).unwrap();
        second.add("C", "c:3").unwrap();
        let all = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(all.fingerprint("a:1"), Some(Fingerprint([1; 32])));
        assert_eq!(all.fingerprint("b:2"), Some(Fingerprint([2; 32])));
        assert!(all.find("c:3").is_some());
        let error = first.remember("b:2", Fingerprint([3; 32])).unwrap_err();
        assert!(error.contains("different key"), "{error}");
        let after = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(after.fingerprint("b:2"), Some(Fingerprint([2; 32])));
        let files: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "no temporary files left: {files:?}");
    }

    /// What the file says is checked as it's read: addresses normalized
    /// (so a key saved by hand for `Example.org` is the one for
    /// `example.org:25575`), and a fingerprint that isn't one or an
    /// address listed twice makes it unreadable, not a server with no key.
    #[test]
    fn a_file_that_cant_be_trusted_is_unreadable() {
        let folder = temp_folder("server-list-checked");
        let path = folder.join(SERVERS_FILE);
        let print = "07".repeat(32);
        std::fs::write(
            &path,
            format!(
                "{{ format: 2, servers: [ {{ address: 'Example.org', fingerprint: '{print}' }} ] }}"
            ),
        )
        .unwrap();
        let list = ServerList::load(Some(&folder)).unwrap();
        assert_eq!(
            list.fingerprint("example.org:25575"),
            Some(Fingerprint([7; 32]))
        );
        for (servers, why) in [
            (
                "{ address: 'a:1', fingerprint: 'beef' }",
                "isn't 64 hex digits",
            ),
            ("{ address: 'a:1', fingerprint: '' }", "isn't 64 hex digits"),
            (
                "{ address: 'a' }, { address: 'A:25575' }",
                "on the list twice",
            ),
            ("{ address: 'a/b' }", "isn't an address"),
        ] {
            std::fs::write(&path, format!("{{ format: 2, servers: [ {servers} ] }}")).unwrap();
            let error = ServerList::load(Some(&folder)).unwrap_err();
            assert!(error.contains(why), "{servers}: {error}");
        }
    }

    /// Without a folder the list is kept in memory.
    #[test]
    fn without_a_folder_it_is_in_memory() {
        let mut list = ServerList::load(None).unwrap();
        list.add("Here", "localhost").unwrap();
        list.remember("localhost:25575", Fingerprint([1; 32]))
            .unwrap();
        assert_eq!(list.servers().len(), 1);
        assert!(list.path().is_none());
    }
}
