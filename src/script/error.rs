//! What went wrong in a script, and where: turning Luau's errors into
//! [`ScriptError`]s that name the mod, the file and the line.

use std::fmt;

/// The kind of thing that went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// The script didn't parse.
    Syntax,
    /// It raised an error while running (including `error(...)` and bad
    /// arguments to the engine's functions).
    Runtime,
    /// A call ran longer than its instruction budget.
    Budget,
    /// The mod's scripts tried to use more memory than the mod's cap.
    Memory,
    /// The script asked for doesn't exist.
    Missing,
}

/// A script's error, as logged and as handed back to whatever called it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// The mod whose scripts were running: the one the failing call or
    /// load belongs to.
    pub mod_id: String,
    /// The script being loaded or called, by id (`mod:name`).
    pub script: String,
    /// The hook being called, or `None` while the script loads.
    pub hook: Option<String>,
    pub kind: ErrorKind,
    /// The file the error happened in, as an asset path
    /// (`mods/<mod>/scripts/<name>.luau`), when Luau knows it. It can be a
    /// script the failing one loaded with `require`.
    pub file: Option<String>,
    /// The line in `file`, counting from 1.
    pub line: Option<u32>,
    /// What went wrong, without the location.
    pub message: String,
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (&self.file, self.line) {
            (Some(file), Some(line)) => write!(f, "{file}:{line}: ")?,
            (Some(file), None) => write!(f, "{file}: ")?,
            _ => write!(f, "{}: ", self.script)?,
        }
        write!(f, "{} (mod `{}`, ", self.message, self.mod_id)?;
        match &self.hook {
            Some(hook) => write!(f, "in `{}` of `{}`)", hook, self.script),
            None => write!(f, "loading `{}`)", self.script),
        }
    }
}

impl std::error::Error for ScriptError {}

/// Why [`super::ScriptRuntime::call`] didn't run a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// This caller failed earlier and stays disabled until the scripts
    /// reload. Its error was logged then; nothing is logged now.
    Disabled,
    /// It failed on this call, and is now disabled. The error has been
    /// logged (or, for a script that didn't load, was logged when it
    /// failed to).
    Failed(Box<ScriptError>),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            CallError::Disabled => write!(f, "disabled by an earlier script error"),
            CallError::Failed(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CallError {}

/// The text Luau gave for an error, its kind and everything after it
/// (including any traceback, which is where an error raised by an engine
/// function finds its line).
pub(super) fn describe(error: &mlua::Error) -> (ErrorKind, String) {
    match error {
        mlua::Error::SyntaxError { message, .. } => (ErrorKind::Syntax, message.clone()),
        mlua::Error::MemoryError(message) => (ErrorKind::Memory, message.clone()),
        mlua::Error::RuntimeError(message) => (ErrorKind::Runtime, message.clone()),
        mlua::Error::CallbackError { traceback, cause } => {
            let (kind, message) = describe(cause);
            (kind, format!("{message}\n{traceback}"))
        }
        mlua::Error::WithContext { context, cause } => {
            let (kind, message) = describe(cause);
            (kind, format!("{context}: {message}"))
        }
        other => (ErrorKind::Runtime, other.to_string()),
    }
}

/// The first `<file>.luau:<line>` in `text`.
pub(super) fn locate(text: &str) -> Option<(String, u32)> {
    let mut from = 0;
    while let Some(found) = text[from..].find(".luau:") {
        let colon = from + found + ".luau".len();
        let digits: String = text[colon + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let start = text[..colon]
            .rfind(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '[' | '('))
            .map_or(0, |i| i + 1);
        if let (Ok(line), false) = (digits.parse(), start == colon) {
            return Some((text[start..colon].to_string(), line));
        }
        from = colon + 1;
    }
    None
}

/// The message without its traceback, or the `file:line: ` Luau starts it
/// with.
pub(super) fn strip(text: &str) -> String {
    let text = text
        .split_once("\nstack traceback:")
        .map_or(text, |(message, _)| message)
        .trim();
    // `mods/x/scripts/y.luau:3: what happened`
    if let Some((file, line)) = locate(text) {
        let prefix = format!("{file}:{line}: ");
        if let Some(rest) = text.strip_prefix(&prefix) {
            return rest.to_string();
        }
    }
    text.to_string()
}

/// Cuts `message` to at most `max` bytes (at a character boundary), saying
/// how long it was, as log lines are, so a huge error isn't kept whole.
pub(super) fn cut(message: &mut String, max: usize) {
    if message.len() <= max {
        return;
    }
    let total = message.len();
    let end = (0..=max)
        .rev()
        .find(|&i| message.is_char_boundary(i))
        .unwrap_or(0);
    message.truncate(end);
    message.push_str(&format!("…({total} bytes)"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_first_file_and_line() {
        assert_eq!(
            locate("mods/test/scripts/a.luau:3: attempt to index nil"),
            Some(("mods/test/scripts/a.luau".into(), 3))
        );
        assert_eq!(
            locate(
                "bad argument #1\nstack traceback:\n\t[C]: in ?\n\tmods/t/scripts/b.luau:12: in function 'f'"
            ),
            Some(("mods/t/scripts/b.luau".into(), 12))
        );
        assert_eq!(locate("not enough memory"), None);
        // A `.luau:` with no line after it isn't a location.
        assert_eq!(
            locate("x.luau: y\nmods/z.luau:4: w"),
            Some(("mods/z.luau".into(), 4))
        );
    }

    #[test]
    fn strips_the_location_and_traceback() {
        assert_eq!(
            strip("mods/t/scripts/a.luau:2: boom\nstack traceback:\n\t[C]: in ?"),
            "boom"
        );
        assert_eq!(strip("not enough memory"), "not enough memory");
    }

    #[test]
    fn errors_read_as_file_line_message_and_mod() {
        let error = ScriptError {
            mod_id: "test".into(),
            script: "test:a".into(),
            hook: Some("on_tick".into()),
            kind: ErrorKind::Runtime,
            file: Some("mods/test/scripts/a.luau".into()),
            line: Some(7),
            message: "boom".into(),
        };
        assert_eq!(
            error.to_string(),
            "mods/test/scripts/a.luau:7: boom (mod `test`, in `on_tick` of `test:a`)"
        );
    }
}
