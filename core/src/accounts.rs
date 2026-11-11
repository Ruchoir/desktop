//! Signing in to an instance with `ruchoir-native`: RFC 8252 with Propolis, PKCE, a loopback
//! redirect (`http://127.0.0.1:<port>/callback`), one refresh token per issuer in the keychain and
//! one access token per product, kept in memory.
//!
//! The sign-in asks a token for the hub and for every installed product at once (`resource`, RFC
//! 8707): the hub's for the live socket and the inbox, the drive's for the local copies. The
//! products' own pages sign in by their own browser session, in their windows.

use std::time::Duration;

use ruchoir_native::{NativeClient, NativeError, SignInTarget};

use crate::instances::Instance;
use crate::vault::Keychain;

/// The app's client at Propolis.
pub const CLIENT_ID: &str = "ruchoir-desktop";

/// How long the browser may take to come back from Propolis.
pub const SIGN_IN_WITHIN: Duration = Duration::from_secs(10 * 60);

/// The client of the app, its grants in the keychain.
pub type Client = NativeClient<Keychain>;

/// A client over `keychain`, with the crate's HTTP client.
pub fn client(keychain: Keychain) -> Client {
    let http = ruchoir_native::http_client().expect("the HTTP client builds");
    NativeClient::new(http, CLIENT_ID, keychain)
}

/// What a sign-in to `instance` asks: its issuer, and a token for each product.
pub fn target(instance: &Instance) -> SignInTarget {
    SignInTarget {
        issuer: instance.issuer.clone(),
        resources: instance.products.iter().map(|p| p.url.clone()).collect(),
    }
}

/// The code the interface translates for a failed step.
pub fn error_code(error: &NativeError) -> &'static str {
    match error {
        NativeError::Discovery(_) => "not_ruchoir",
        NativeError::Network(_) => "unreachable",
        NativeError::SessionEnded | NativeError::SignedOut => "signed_out",
        NativeError::Storage(_) => "keychain",
        NativeError::Browser(_) => "browser",
        _ => "sign_in_failed",
    }
}

/// The page the browser shows once it is back on the loopback listener, in the person's language
/// (`lang`: a BCP 47 tag, `fr` when unknown, French being the source).
pub fn confirmation_page(lang: &str) -> String {
    let primary = lang
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (code, title, body) = match primary.as_str() {
        "en" => (
            "en",
            "You are signed in",
            "You can close this tab and go back to Ruchoir.",
        ),
        "es" => (
            "es",
            "Has iniciado sesión",
            "Puedes cerrar esta pestaña y volver a Ruchoir.",
        ),
        "de" => (
            "de",
            "Du bist angemeldet",
            "Du kannst diesen Tab schließen und zu Ruchoir zurückkehren.",
        ),
        "it" => (
            "it",
            "Accesso effettuato",
            "Puoi chiudere questa scheda e tornare a Ruchoir.",
        ),
        "pl" => (
            "pl",
            "Zalogowano",
            "Możesz zamknąć tę kartę i wrócić do Ruchoir.",
        ),
        _ => (
            "fr",
            "Vous êtes connecté",
            "Vous pouvez fermer cet onglet et revenir à Ruchoir.",
        ),
    };
    format!(
        "<!doctype html><html lang=\"{code}\"><head><meta charset=\"utf-8\"><title>Ruchoir</title>\
         <style>body{{font-family:system-ui,sans-serif;display:grid;place-items:center;min-height:90vh;margin:0}}\
         main{{text-align:center}}</style></head><body><main><h1>{title}</h1><p>{body}</p></main></body></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instances::InstalledProduct;

    #[test]
    fn the_page_speaks_the_persons_language_and_falls_back_to_french() {
        assert!(confirmation_page("en-GB").contains("You are signed in"));
        assert!(confirmation_page("de").contains("lang=\"de\""));
        assert!(confirmation_page("pl_PL").contains("Zalogowano"));
        assert!(confirmation_page("es").contains("Has iniciado"));
        assert!(confirmation_page("it").contains("Accesso"));
        assert!(confirmation_page("ja").contains("Vous êtes connecté"));
        assert!(confirmation_page("").contains("lang=\"fr\""));
    }

    #[test]
    fn a_sign_in_asks_a_token_for_every_product() {
        let instance = Instance {
            id: "a".into(),
            domain: "example.org".into(),
            issuer: "https://auth.example.org".into(),
            products: ["hub", "drive"]
                .iter()
                .map(|n| InstalledProduct {
                    name: (*n).into(),
                    url: format!("https://{n}.example.org"),
                })
                .collect(),
            added_at: 0,
        };
        let target = target(&instance);
        assert_eq!(target.issuer, "https://auth.example.org");
        assert_eq!(
            target.resources,
            ["https://hub.example.org", "https://drive.example.org"]
        );
    }

    #[test]
    fn errors_are_codes() {
        assert_eq!(error_code(&NativeError::SessionEnded), "signed_out");
        assert_eq!(error_code(&NativeError::Network("x".into())), "unreachable");
    }
}
