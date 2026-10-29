//! Executor evidence of the server fixture. The `qrow_block` function of
//! `tests/fixture/server/Blocking.java` appends a line to `{token}.{state}`
//! in a directory that the fixture shares with the host. The fixture gives
//! that directory in `QROW_E2E_NATIVE_EVIDENCE` for both runtimes.
use std::{
    io,
    path::{Path, PathBuf},
};

const STATES: [&str; 4] = ["started", "interrupted", "completed", "ended"];

/// The evidence directory of the fixture.
pub fn directory() -> PathBuf {
    std::env::var_os("QROW_E2E_NATIVE_EVIDENCE")
        .expect("The fixture did not give an evidence directory. Run tests through ./qtest.")
        .into()
}

fn word(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// How often the executor recorded `token.state` in `root`. The driver
/// records `ended` under the Spark task of the token.
pub fn count_in(root: &Path, token: &str, state: &str) -> io::Result<usize> {
    if !word(token) || !STATES.contains(&state) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid evidence name: {token}.{state}"),
        ));
    }
    let name = if state == "ended" {
        let task = std::fs::read_to_string(root.join(format!("{token}.task")))?;
        let task = task.trim();
        if !task.strip_prefix("app-").is_some_and(word) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Invalid Spark task reference: {task}"),
            ));
        }
        format!("{task}.ended")
    } else {
        format!("{token}.{state}")
    };
    match std::fs::read_to_string(root.join(name)) {
        Ok(text) => Ok(text.lines().count()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// How often the executor recorded `token.state`: started, interrupted,
/// completed, or ended.
pub fn count(token: &str, state: &str) -> io::Result<usize> {
    count_in(&directory(), token, state)
}
