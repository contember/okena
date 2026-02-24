/// App registry — validates known app kinds.

/// Known app kind entry.
pub struct AppKind {
    pub id: &'static str,
}

static KNOWN_APPS: &[AppKind] = &[
    AppKind { id: "kruh" },
];

/// Look up an app kind by ID.
pub fn find_app(kind: &str) -> Option<&'static AppKind> {
    KNOWN_APPS.iter().find(|a| a.id == kind)
}
