//! Session settings that determine the meaning of the server's result text.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Context {
    pub postgres: Option<Postgres>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Postgres {
    pub date_style: String,
    pub interval_style: String,
    pub time_zone: String,
}

impl Context {
    pub fn iso_dates(&self) -> bool {
        self.postgres.as_ref().is_none_or(|settings| {
            settings
                .date_style
                .split(',')
                .next()
                .is_some_and(|style| style.trim() == "ISO")
        })
    }
}
