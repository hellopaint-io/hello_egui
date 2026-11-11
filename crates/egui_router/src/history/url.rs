//! Mapping between browser locations and routes, kept free of `web_sys` so it
//! can be tested natively.

/// The route a browser location points at.
///
/// A base href containing `#` keeps routes in the fragment. The path and query
/// before the `#` then address the page itself, not the route, so they are
/// ignored: a page served at `/repo/?api=x#/gallery` with base href `/repo/#`
/// is on `/gallery`.
pub(crate) fn route_from_location(
    base_href: &str,
    pathname: &str,
    search: &str,
    hash: &str,
) -> String {
    if let Some((_, fragment_prefix)) = base_href.split_once('#') {
        let fragment = hash.strip_prefix('#').unwrap_or(hash);
        let route = fragment.strip_prefix(fragment_prefix).unwrap_or(fragment);
        return if route.is_empty() {
            "/".to_owned()
        } else {
            route.to_owned()
        };
    }

    let full_path = format!("{pathname}{search}{hash}");
    if base_href.starts_with(&full_path) {
        "/".to_owned()
    } else {
        full_path
            .strip_prefix(base_href)
            .unwrap_or(&full_path)
            .to_owned()
    }
}

/// The URL to put in the address bar for `route`.
///
/// With routes in the fragment, the page's current query (`search`, including
/// its `?`) is kept, so navigating doesn't drop parameters the page was opened
/// with.
pub(crate) fn url_for_route(base_href: &str, search: &str, route: &str) -> String {
    match base_href.split_once('#') {
        Some((page, fragment_prefix)) => format!("{page}{search}#{fragment_prefix}{route}"),
        None => format!("{base_href}{route}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_routes_ignore_the_page_path_and_query() {
        let base = "/previews/main/#";
        assert_eq!(route_from_location(base, "/previews/main/", "", ""), "/");
        assert_eq!(
            route_from_location(base, "/previews/main/", "?api=production", ""),
            "/"
        );
        assert_eq!(
            route_from_location(base, "/previews/main/", "?api=production", "#/gallery"),
            "/gallery"
        );
        assert_eq!(
            route_from_location(base, "/previews/main/", "", "#/gallery?tab=new"),
            "/gallery?tab=new"
        );
        // Served without the trailing slash, e.g. after a redirect.
        assert_eq!(
            route_from_location(base, "/previews/main", "", "#/gallery"),
            "/gallery"
        );
    }

    #[test]
    fn path_routes_strip_the_base() {
        assert_eq!(route_from_location("", "/", "", ""), "/");
        assert_eq!(
            route_from_location("", "/gallery", "?q=a", ""),
            "/gallery?q=a"
        );
        assert_eq!(
            route_from_location("/app", "/app/gallery", "", ""),
            "/gallery"
        );
        assert_eq!(route_from_location("/app/", "/app", "", ""), "/");
    }

    #[test]
    fn urls_keep_the_page_query_with_fragment_routes() {
        assert_eq!(
            url_for_route("/previews/main/#", "?api=production", "/gallery"),
            "/previews/main/?api=production#/gallery"
        );
        assert_eq!(
            url_for_route("/previews/main/#", "", "/gallery"),
            "/previews/main/#/gallery"
        );
        assert_eq!(url_for_route("", "?api=production", "/gallery"), "/gallery");
        assert_eq!(url_for_route("/app", "", "/gallery"), "/app/gallery");
    }
}
