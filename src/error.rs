//! What fails, by where it fails.

/// A failure. The chat says it as `Failed to <verb>: <error>`, the journal
/// as `error: <error>`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The homeserver cannot be reached
    #[error("cannot reach the homeserver: {0}")]
    Transport(String),
    /// The homeserver answered with an error status; `code` is its Matrix
    /// error code, else the status text
    #[error("the homeserver answered {status} {code}")]
    Http {
        status: u16,
        code: String,
        body: Vec<u8>,
    },
    /// herdr ran and failed; what it said
    #[error("herdr: {0}")]
    Herdr(String),
    /// A program the bot runs failed
    #[error("{0}")]
    Tool(String),
    /// Data from herdr or Matrix the bot cannot read: a bug
    #[error("unexpected data: {0}")]
    Malformed(String),
    #[error("there is no chat yet")]
    NoChat,
    /// What the owner asked cannot be done
    #[error("{0}")]
    Refused(String),
    /// A setting is missing or wrong
    #[error("{0}")]
    Setting(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// The Matrix error code of a failed request.
    #[must_use]
    pub fn errcode(&self) -> Option<String> {
        let Self::Http { body, .. } = self else {
            return None;
        };
        let value: serde_json::Value = serde_json::from_slice(body).ok()?;
        value.get("errcode")?.as_str().map(str::to_owned)
    }

    /// What the owner can do about it, where there is something.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Http { status: 401, .. } => {
                Some("Check HERDR_MATRIX_TOKEN: it must be the as_token of the appservice.")
            }
            Self::Http { .. } if self.errcode().as_deref() == Some("M_EXCLUSIVE") => {
                Some("Check the user namespace of the appservice registration.")
            }
            Self::Herdr(_) => Some("Is herdr running?"),
            Self::Malformed(_) => Some("This is a bug; the journal has the details."),
            _ => None,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Malformed(err.to_string())
    }
}
