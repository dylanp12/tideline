use serde::Serialize;

/// One chunk of streamed output with its position in the stream.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Token {
    pub offset: u64,
    pub data: String,
}

/// What gets broadcast to live subscribers.
#[derive(Clone, Debug)]
pub enum Signal {
    Token(Token),
    /// The requested resume offset was already evicted from the buffer; the
    /// client should expect a discontinuity (and may restart from the tail).
    Gap,
    /// The producer terminated the stream with an error. Terminal.
    Error(String),
    /// The stream finished successfully. Terminal.
    Done,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_serializes_to_json() {
        let t = Token {
            offset: 3,
            data: "hi".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"offset":3,"data":"hi"}"#);
    }
}
