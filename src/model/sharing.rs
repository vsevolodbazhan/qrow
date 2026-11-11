//! Connection settings as text that a user copies to another user or to a
//! file. The text has no identifiers, passwords, tokens, or accounts.

use super::{
    Authentication, CatalogColumnReads, CatalogSettings, ConnectionLifecycle, DatabaseType,
    PostgresSslMode, Profile, SharedSignIn, SignIn, SignInProvider, shared_object,
    transfer::Transfer, unique_sign_in_name,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The format version of shared connection settings.
pub const SHARED_CONNECTION_VERSION: u32 = 1;
/// The format version of a connections file.
pub const CONNECTIONS_FILE_VERSION: u32 = 1;
/// The longest text that Qrow reads as the settings of one connection.
const MAX_SHARED_CONNECTION_BYTES: usize = 256 * 1024;
/// The largest connections file that Qrow reads.
pub const MAX_CONNECTIONS_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// The most connections in one connections file.
const MAX_FILE_CONNECTIONS: usize = 1_000;

const NOT_A_CONNECTION: &str = "The clipboard does not contain connection settings. Use Copy settings on a connection, then try again.";
const NOT_A_FILE: &str =
    "The file does not contain connections. Choose a file from Export connections.";

/// The settings of a connection that one user gives to another. Each field
/// is named here, so that a new private field of [`Profile`] does not go into
/// the text by accident. The dbt project and the shared catalog stay out,
/// because they refer to files and catalogs on this computer.
///
/// `S` is the sign-in: its settings in the text, or a JSON value that
/// [`SharedSignIn::read`] checks while Qrow reads the text.
#[derive(Deserialize, Serialize)]
struct SharedConnection<S = SharedSignIn> {
    /// Marks the object as connection settings and gives the format version.
    #[serde(default)]
    qrow_connection: u32,
    name: String,
    #[serde(default)]
    database_type: DatabaseType,
    host: String,
    port: u16,
    username: String,
    database: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    trino_schema: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_default")]
    tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    postgres_ssl_mode: Option<PostgresSslMode>,
    #[serde(default, skip_serializing_if = "SharedAuthentication::is_password")]
    authentication: SharedAuthentication<S>,
    #[serde(default, skip_serializing_if = "is_default")]
    transfer: Transfer,
    #[serde(default, skip_serializing_if = "is_default")]
    lifecycle: ConnectionLifecycle,
    #[serde(default, skip_serializing_if = "is_default")]
    catalog: CatalogSettings,
    #[serde(default, skip_serializing_if = "is_default")]
    catalog_column_reads: CatalogColumnReads,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    assistant_notes: String,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// How a shared connection authenticates. A sign-in goes with its settings,
/// because its identifier means nothing on another computer.
#[derive(Default, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case")]
enum SharedAuthentication<S> {
    #[default]
    Password,
    Oidc {
        sign_in: S,
    },
    TrinoExternal,
}

impl<S> SharedAuthentication<S> {
    fn is_password(&self) -> bool {
        matches!(self, Self::Password)
    }
}

/// A connection read from shared settings. It has a new identifier and no
/// password.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedConnectionSettings {
    pub profile: Profile,
    /// The sign-in of the connection. `profile.authentication` refers to it.
    pub sign_in: Option<SignIn>,
}

impl SharedConnection {
    fn of(profile: &Profile, catalog: &CatalogSettings, sign_in: Option<&SignIn>) -> Self {
        let authentication = match (profile.authentication, sign_in) {
            (Authentication::Oidc { .. }, Some(sign_in)) => SharedAuthentication::Oidc {
                sign_in: SharedSignIn::of(sign_in),
            },
            (Authentication::TrinoExternal, _) => SharedAuthentication::TrinoExternal,
            _ => SharedAuthentication::Password,
        };
        Self {
            qrow_connection: SHARED_CONNECTION_VERSION,
            name: profile.name.clone(),
            database_type: profile.database_type,
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            database: profile.database.clone(),
            trino_schema: profile.trino_schema.clone(),
            parameters: profile.parameters.clone(),
            tls: profile.tls,
            // Profiles before the Postgres TLS modes keep their mode.
            postgres_ssl_mode: (profile.database_type == DatabaseType::Postgres)
                .then(|| profile.postgres_ssl_mode()),
            authentication,
            transfer: profile.transfer.clone(),
            lifecycle: profile.lifecycle.clone(),
            catalog: catalog.clone(),
            catalog_column_reads: profile.catalog_column_reads,
            assistant_notes: profile.assistant_notes.clone(),
        }
    }
}

impl SharedConnection<serde_json::Value> {
    /// A new connection from the shared settings in `value`.
    fn read(value: serde_json::Value) -> anyhow::Result<SharedConnectionSettings> {
        let version = value
            .get("qrow_connection")
            .map_or(Some(0), serde_json::Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("The connection settings have no format version."))?;
        anyhow::ensure!(
            version <= u64::from(SHARED_CONNECTION_VERSION),
            "These connection settings come from a newer version of Qrow. Update Qrow, then try again."
        );
        let shared: Self = serde_json::from_value(value).map_err(|error| {
            anyhow::anyhow!("The connection settings are not complete: {error}.")
        })?;
        let (authentication, sign_in) = match shared.authentication {
            SharedAuthentication::Password => (Authentication::Password, None),
            SharedAuthentication::TrinoExternal => (Authentication::TrinoExternal, None),
            SharedAuthentication::Oidc { sign_in } => {
                let sign_in = SharedSignIn::read(sign_in)?;
                anyhow::ensure!(
                    sign_in.provider == SignInProvider::Oidc,
                    "The sign-in of the connection must use OpenID Connect."
                );
                (
                    Authentication::Oidc {
                        sign_in: sign_in.id,
                    },
                    Some(sign_in),
                )
            }
        };
        let profile = Profile {
            database_type: shared.database_type,
            name: shared.name.trim().to_owned(),
            host: shared.host.trim().to_owned(),
            port: shared.port,
            username: shared.username.trim().to_owned(),
            database: shared.database.trim().to_owned(),
            trino_schema: shared.trino_schema.trim().to_owned(),
            parameters: shared.parameters,
            transfer: shared.transfer,
            lifecycle: shared.lifecycle,
            catalog: shared.catalog,
            catalog_column_reads: shared.catalog_column_reads,
            shared_catalog: None,
            tls: shared.tls,
            postgres_ssl_mode: shared.postgres_ssl_mode,
            authentication,
            assistant_notes: shared.assistant_notes.trim().to_owned(),
            dbt: None,
            ..Profile::default()
        };
        let mut profile = profile;
        if profile.database_type == DatabaseType::Postgres {
            profile.postgres_ssl_mode = Some(profile.postgres_ssl_mode());
        }
        profile.validate()?;
        Ok(SharedConnectionSettings { profile, sign_in })
    }
}

/// The connections of a connections file.
#[derive(Serialize)]
struct ConnectionsFile<'a> {
    qrow_connections: u32,
    connections: &'a [SharedConnection],
}

impl Profile {
    /// The settings of this connection as text that another user can paste.
    /// `catalog` gives the schema settings that apply to the connection, and
    /// `sign_in` is its sign-in, if any. The password stays in the secret
    /// store.
    pub fn to_shared_text(&self, catalog: &CatalogSettings, sign_in: Option<&SignIn>) -> String {
        serde_json::to_string_pretty(&SharedConnection::of(self, catalog, sign_in))
            .expect("connection settings serialize")
    }

    /// Reads settings from [`Self::to_shared_text`] into a new connection
    /// without a password. Text around the settings, like the rest of a chat
    /// message, and typographic quotes are accepted.
    pub fn from_shared_text(text: &str) -> anyhow::Result<SharedConnectionSettings> {
        anyhow::ensure!(text.len() <= MAX_SHARED_CONNECTION_BYTES, NOT_A_CONNECTION);
        let value = shared_object(text, "qrow_connection")
            .filter(|value| {
                value
                    .get("qrow_connection")
                    .is_some_and(serde_json::Value::is_u64)
            })
            .ok_or_else(|| anyhow::anyhow!(NOT_A_CONNECTION))?;
        SharedConnection::<serde_json::Value>::read(value)
    }
}

/// The text of a connections file with `connections`: each connection with
/// its schema settings and its sign-in.
pub fn connections_file_text<'a>(
    connections: impl IntoIterator<Item = (&'a Profile, CatalogSettings, Option<&'a SignIn>)>,
) -> String {
    let connections: Vec<_> = connections
        .into_iter()
        .map(|(profile, catalog, sign_in)| SharedConnection::of(profile, &catalog, sign_in))
        .collect();
    let mut text = serde_json::to_string_pretty(&ConnectionsFile {
        qrow_connections: CONNECTIONS_FILE_VERSION,
        connections: &connections,
    })
    .expect("connections serialize");
    text.push('\n');
    text
}

/// Reads the connections of a connections file. A file with the settings of
/// one connection, as Copy settings makes them, is also accepted.
pub fn read_connections_file(text: &str) -> anyhow::Result<Vec<SharedConnectionSettings>> {
    let value: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| anyhow::anyhow!("The file is not valid JSON: {error}."))?;
    if value.get("qrow_connection").is_some() {
        return Ok(vec![SharedConnection::<serde_json::Value>::read(value)?]);
    }
    let version = value
        .get("qrow_connections")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!(NOT_A_FILE))?;
    anyhow::ensure!(
        version <= u64::from(CONNECTIONS_FILE_VERSION),
        "This file comes from a newer version of Qrow. Update Qrow, then try again."
    );
    let connections = value
        .get("connections")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!(NOT_A_FILE))?;
    anyhow::ensure!(
        !connections.is_empty(),
        "The file does not contain connections."
    );
    anyhow::ensure!(
        connections.len() <= MAX_FILE_CONNECTIONS,
        "The file contains more than {MAX_FILE_CONNECTIONS} connections."
    );
    connections
        .iter()
        .enumerate()
        .map(|(index, connection)| {
            SharedConnection::<serde_json::Value>::read(connection.clone()).map_err(|error| {
                let name = connection
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(|name| format!(" \"{}\"", name.trim()))
                    .unwrap_or_default();
                anyhow::anyhow!("Connection {}{name}: {error}", index + 1)
            })
        })
        .collect()
}

/// Shared connections, ready to add to a workspace with `profiles` and
/// `sign_ins`.
#[derive(Debug, Default, PartialEq)]
pub struct PlacedConnections {
    /// The new connections, with names that no other connection has.
    pub profiles: Vec<Profile>,
    /// The sign-ins that the workspace does not have yet.
    pub sign_ins: Vec<SignIn>,
}

/// Gives each shared connection a free name, and a sign-in of the workspace
/// when one has the same provider settings. Other sign-ins are new, with free
/// names, and connections with the same sign-in settings share one.
pub fn place_connections(
    connections: Vec<SharedConnectionSettings>,
    profiles: &[Profile],
    sign_ins: &[SignIn],
) -> PlacedConnections {
    let mut placed = PlacedConnections::default();
    for SharedConnectionSettings {
        mut profile,
        sign_in,
    } in connections
    {
        if let Some(sign_in) = sign_in {
            let id = match sign_ins
                .iter()
                .chain(&placed.sign_ins)
                .find(|existing| existing.same_provider_settings(&sign_in))
            {
                Some(existing) => existing.id,
                None => {
                    let name = unique_sign_in_name(&sign_in.name, |name| {
                        sign_ins
                            .iter()
                            .chain(&placed.sign_ins)
                            .any(|other| other.name == name)
                    });
                    let id = sign_in.id;
                    placed.sign_ins.push(SignIn { name, ..sign_in });
                    id
                }
            };
            profile.authentication = Authentication::Oidc { sign_in: id };
        }
        profile.name = super::unique_profile_name(&profile.name, |name| {
            profiles
                .iter()
                .chain(&placed.profiles)
                .any(|other| other.name == name)
        });
        placed.profiles.push(profile);
    }
    placed
}

#[cfg(test)]
mod tests;
