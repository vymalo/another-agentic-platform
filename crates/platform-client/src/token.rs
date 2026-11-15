//! The bearer token, in a type that cannot be printed by accident.

use std::fmt;

use crate::ClientError;

/// An access token. `Debug` shows nothing of it and there is no `Display`: the one way out is [`Token::expose`], which the client
/// uses for the `Authorization` header and `aapctl` for the 0600 file.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// A token from text: surrounding white space is dropped, and what is left must be visible ASCII with no white space inside (a
    /// token with a line break in it would add a header to the request).
    ///
    /// # Errors
    ///
    /// [`ClientError::Config`], whose message never holds the text.
    pub fn new(text: &str) -> Result<Self, ClientError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ClientError::Config("the token is empty".to_owned()));
        }
        if !text.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ClientError::Config(
                "the token has white space, a control character or a character that is not ASCII in it".to_owned(),
            ));
        }
        Ok(Self(text.to_owned()))
    }

    /// The token's text. Only for the header and the file.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(…)")
    }
}
