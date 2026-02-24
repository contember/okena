/// Marker trait for app view states — must be serializable to JSON
pub trait AppViewState: serde::Serialize + serde::de::DeserializeOwned + Send + 'static {}

/// Marker trait for app actions — must be serializable to JSON
pub trait AppAction: serde::Serialize + serde::de::DeserializeOwned + Send + 'static {}
