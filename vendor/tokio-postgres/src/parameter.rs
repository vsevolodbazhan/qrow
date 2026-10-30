//! Bound server session metadata before retaining strings.
use crate::Error;
use std::{collections::HashMap, io};

pub(crate) fn insert(
    parameters: &mut HashMap<String, String>,
    name: &str,
    value: &str,
) -> Result<(), Error> {
    let previous = parameters.get(name).map_or(0, |old| name.len() + old.len());
    let bytes = parameters
        .iter()
        .map(|(key, value)| key.len() + value.len())
        .sum::<usize>();
    if name.len() > 1024
        || value.len() > 1024
        || (!parameters.contains_key(name) && parameters.len() >= 4096)
        || bytes - previous + name.len() + value.len() > 2 * 1024 * 1024
    {
        return Err(Error::parse(
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Postgres session metadata exceeds its limit",
            )
            .into(),
        ));
    }
    parameters.insert(name.to_owned(), value.to_owned());
    Ok(())
}
