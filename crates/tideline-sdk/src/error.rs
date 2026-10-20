use tideline_proto::ChainError;

#[derive(Debug)]
pub enum Error {
    /// The request never completed: DNS, connection, TLS, timeout.
    Transport(reqwest::Error),
    /// The server answered, and said no. The body is carried because TLR/1
    /// error bodies name the reason ("run is sealed", "gate 4 is already
    /// resolved") and swallowing that would make the SDK useless to debug.
    Status { code: u16, body: String },
    /// The server answered with something this protocol version cannot read.
    Protocol(String),
    /// A record failed verification, naming the event at fault.
    Chain(ChainError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "transport: {e}"),
            Error::Status { code, body } => write!(f, "server returned {code}: {body}"),
            Error::Protocol(m) => write!(f, "protocol: {m}"),
            Error::Chain(e) => write!(f, "record does not verify: {e:?}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Transport(e)
    }
}

impl From<ChainError> for Error {
    fn from(e: ChainError) -> Self {
        Error::Chain(e)
    }
}

impl Error {
    /// True when the server refused because the record's state forbids it —
    /// a sealed run, a gate already decided. Callers branch on this.
    pub fn is_conflict(&self) -> bool {
        matches!(self, Error::Status { code: 409, .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Status { code: 404, .. })
    }
}

pub type Result<T> = std::result::Result<T, Error>;
