//! Where a product's window may go. It shows the instance and nothing else: its products' origins
//! and Propolis's (the product's own sign-in passes through it). A link to another site opens in the
//! system browser, so a remote page never sits in a Ruchoir window under the app's name.

/// What to do with a navigation of a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Navigation {
    /// One of the instance's origins (or a blank or blob page it made): go there.
    Stay,
    /// A web page or a mail address: hand it to the system, stay put.
    OpenOutside,
    /// Anything else (files, scripts, other schemes): do nothing.
    Refuse,
}

/// Decide a navigation of a window of the instance whose origins are `origins` (compared exactly:
/// scheme, host and port, no suffix, no wildcard).
pub fn navigation(url: &url::Url, origins: &[String]) -> Navigation {
    match url.scheme() {
        "http" | "https" if origins.contains(&url.origin().ascii_serialization()) => {
            Navigation::Stay
        }
        "about" if url.path() == "blank" => Navigation::Stay,
        "blob" => Navigation::Stay,
        "http" | "https" | "mailto" => Navigation::OpenOutside,
        _ => Navigation::Refuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide(url: &str) -> Navigation {
        let origins = [
            "https://auth.example.org".to_owned(),
            "https://drive.example.org".to_owned(),
        ];
        navigation(&url::Url::parse(url).unwrap(), &origins)
    }

    #[test]
    fn the_instances_origins_stay_and_the_rest_goes_outside() {
        assert_eq!(decide("https://drive.example.org/f/1"), Navigation::Stay);
        assert_eq!(decide("https://auth.example.org/sign-in"), Navigation::Stay);
        assert_eq!(decide("about:blank"), Navigation::Stay);
        assert_eq!(decide("https://example.com/"), Navigation::OpenOutside);
        assert_eq!(
            decide("https://drive.example.org.evil.test/"),
            Navigation::OpenOutside
        );
        assert_eq!(decide("http://drive.example.org/"), Navigation::OpenOutside);
        assert_eq!(
            decide("https://drive.example.org:8443/"),
            Navigation::OpenOutside
        );
        assert_eq!(decide("mailto:a@example.org"), Navigation::OpenOutside);
        assert_eq!(decide("file:///etc/passwd"), Navigation::Refuse);
        assert_eq!(decide("javascript:alert(1)"), Navigation::Refuse);
    }
}
