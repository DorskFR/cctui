//! The HTTP clients expect the process to have installed a rustls
//! `CryptoProvider`.

mod creds;
pub mod sigv4;

pub use creds::{
    AmbientAws, AwsCredentials, Clock, CredentialSource, DefaultChain, EnvLookup, FixedSource,
};
