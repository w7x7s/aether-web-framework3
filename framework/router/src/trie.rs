//! # Segment Trie Router with Single-Instruction Bitmask Method Matching
//!
//! Paths are split into `/`-separated segments and matched against a trie where each segment
//! consumes exactly one node. This is *not* a compressed radix/prefix tree: there is no edge
//! splitting, so a lookup costs O(segments x siblings), not O(segments). The sibling count per
//! node is small and the scan stays allocation-free on the hot path (params borrow from the
//! request path), which is what actually matters for HTTP routing. If prefix compression ever
//! lands, the name and the complexity claim can meet again.

use crate::node::{HandlerId, NodeType, RouteNode};
use crate::params::{Param, PathParams};
use aether_http::method::Method;

pub struct RadixRouter {
    root: RouteNode,
}

#[derive(Debug)]
pub enum MatchResult<'a> {
    Found {
        handler_id: HandlerId,
        params: PathParams<'a>,
    },
    MethodNotAllowed {
        allowed_bitmask: u64,
    },
    NotFound,
}

impl RadixRouter {
    pub fn new() -> Self {
        Self {
            root: RouteNode::new("/".to_string(), NodeType::Static),
        }
    }

    /// Converts a method bitmask to a comma-separated list of HTTP methods for the Allow header
    pub fn bitmask_to_allow_header(bitmask: u64) -> String {
        let mut methods = Vec::new();
        if bitmask & Method::Get.bitmask() != 0 {
            methods.push("GET");
        }
        if bitmask & Method::Post.bitmask() != 0 {
            methods.push("POST");
        }
        if bitmask & Method::Put.bitmask() != 0 {
            methods.push("PUT");
        }
        if bitmask & Method::Delete.bitmask() != 0 {
            methods.push("DELETE");
        }
        if bitmask & Method::Patch.bitmask() != 0 {
            methods.push("PATCH");
        }
        if bitmask & Method::Head.bitmask() != 0 {
            methods.push("HEAD");
        }
        if bitmask & Method::Options.bitmask() != 0 {
            methods.push("OPTIONS");
        }
        if bitmask & Method::Trace.bitmask() != 0 {
            methods.push("TRACE");
        }
        if bitmask & Method::Connect.bitmask() != 0 {
            methods.push("CONNECT");
        }
        methods.join(", ")
    }

    /// تسجيل مسار جديد داخل شجرة التوجيه (مثل `/api/v1/users/{id}` أو `/static/*filepath`).
    /// 
    /// يتم تحليل المقاطع وإنشاء العقد الفرعية المناسبة مع فرزها لضمان أسبقية المطابقة الحتمية.
    pub fn add_route(&mut self, method: Method, path: &str, handler: HandlerId) {
        let segments = path.trim_matches('/').split('/').filter(|s| !s.is_empty());
        let mut curr = &mut self.root;

        if path == "/" || path.is_empty() {
            Self::reject_duplicate(curr, method, path);
            curr.add_handler(method, handler);
            return;
        }

        for seg in segments {
            let (node_type, is_catchall) = if seg.starts_with('{') && seg.ends_with('}') {
                let name = &seg[1..seg.len() - 1];
                (NodeType::Param(name.to_string()), false)
            } else if seg.starts_with('*') {
                let name = if seg.len() > 1 { &seg[1..] } else { "wildcard" };
                (NodeType::CatchAll(name.to_string()), true)
            } else {
                (NodeType::Static, false)
            };

            // A node level may hold at most one Param child and one CatchAll child,
            // regardless of the variable's *name*. Matching by name (the previous
            // behaviour) let `/users/{id}` and `/users/{user}` coexist as two siblings;
            // they compete for the exact same request segments, so the second was
            // silently shadowed and `reject_duplicate` never fired. Matching by kind
            // collapses them onto the first-registered node, turning a duplicate method
            // into a startup panic instead of an unreachable handler. The first name wins.
            let child_idx = curr.children.iter().position(|c| match (&c.node_type, &node_type) {
                (NodeType::Static, NodeType::Static) => c.prefix == seg,
                (NodeType::Param(_), NodeType::Param(_)) => true,
                (NodeType::CatchAll(_), NodeType::CatchAll(_)) => true,
                _ => false,
            });

            if let Some(idx) = child_idx {
                curr = &mut curr.children[idx];
            } else {
                let new_node = RouteNode::new(seg.to_string(), node_type);
                curr.children.push(new_node);

                // فرز العقد الفرعية لضمان الأسبقية الحتمية: Static أولاً، ثم Param، ثم CatchAll
                curr.children.sort_by_key(|c| match &c.node_type {
                    NodeType::Static => 0,
                    NodeType::Param(_) => 1,
                    NodeType::CatchAll(_) => 2,
                });

                let pos = curr.children.iter().position(|c| c.prefix == seg).unwrap();
                curr = &mut curr.children[pos];
            }

            if is_catchall {
                break;
            }
        }

        Self::reject_duplicate(curr, method, path);
        curr.add_handler(method, handler);
    }

    /// Duplicate `method + path` registration is a programming error. Silently ignoring the
    /// second registration (the previous behaviour) hides the mistake until runtime, so fail
    /// loudly at startup the way Axum and Actix Web do.
    fn reject_duplicate(node: &RouteNode, method: Method, path: &str) {
        if node.supports_method(method) {
            panic!(
                "Aether: duplicate route registration for {} '{}'; each method+path pair must be unique",
                method.as_str(),
                path
            );
        }
    }

    /// مطابقة المسار وأسلوب HTTP.
    ///
    /// Matching answers two separate questions: does the *path* exist, and is the *method*
    /// allowed on it. Keeping them apart is what makes backtracking correct — a static sibling
    /// that matches the path but not the method must not mask a parameter route that matches
    /// both. A full (path + method) match always wins over any partial one.
    pub fn find<'a>(&'a self, method: Method, path: &'a str) -> MatchResult<'a> {
        let mut params = PathParams::new();
        let trimmed = path.trim_matches('/');

        if trimmed.is_empty() {
            return self.leaf_result(&self.root, method, params);
        }

        // استخدام SmallVec على الـ Stack لتفادي تخصيص مصفوفة على الـ Heap
        let mut segments: smallvec::SmallVec<[&'a str; 8]> = smallvec::SmallVec::new();
        for seg in trimmed.split('/') {
            if !seg.is_empty() {
                segments.push(seg);
            }
        }

        // Accumulate the methods of *every* branch whose path matched. `/users/me` may exist
        // as a static GET route and a param POST route simultaneously; RFC 9110 §15.5.6
        // requires the 405 to advertise all methods the target resource supports, so the
        // union — not the first branch encountered — is the correct Allow value.
        let mut allowed_bitmask: u64 = 0;
        match self.search(
            &self.root,
            method,
            trimmed,
            &segments,
            0,
            &mut params,
            &mut allowed_bitmask,
        ) {
            Search::Found { handler_id, params } => MatchResult::Found { handler_id, params },
            Search::NoMatch if allowed_bitmask != 0 => {
                MatchResult::MethodNotAllowed { allowed_bitmask }
            }
            Search::NoMatch => MatchResult::NotFound,
        }
    }

    fn leaf_result<'a>(
        &'a self,
        node: &'a RouteNode,
        method: Method,
        params: PathParams<'a>,
    ) -> MatchResult<'a> {
        if let Some(handler_id) = node.get_handler(method) {
            MatchResult::Found { handler_id, params }
        } else if node.allowed_methods != 0 {
            MatchResult::MethodNotAllowed {
                allowed_bitmask: node.allowed_methods,
            }
        } else {
            MatchResult::NotFound
        }
    }

    fn search<'a>(
        &'a self,
        curr: &'a RouteNode,
        method: Method,
        path: &'a str,
        segments: &[&'a str],
        index: usize,
        params: &mut PathParams<'a>,
        allowed_bitmask: &mut u64,
    ) -> Search<'a> {
        if index == segments.len() {
            if let Some(handler_id) = curr.get_handler(method) {
                return Search::Found {
                    handler_id,
                    params: params.clone(),
                };
            }
            *allowed_bitmask |= curr.allowed_methods;
            return Search::NoMatch;
        }

        let seg = segments[index];

        // 1. Static children first: an exact literal beats any dynamic match.
        for child in &curr.children {
            if let NodeType::Static = &child.node_type {
                if child.prefix == seg {
                    let res =
                        self.search(child, method, path, segments, index + 1, params, allowed_bitmask);
                    if matches!(res, Search::Found { .. }) {
                        return res;
                    }
                }
            }
        }

        // 2. Single-segment parameters.
        for child in &curr.children {
            if let NodeType::Param(key) = &child.node_type {
                params.push(Param {
                    key: key.as_str(),
                    value: seg,
                });
                let res =
                    self.search(child, method, path, segments, index + 1, params, allowed_bitmask);
                if matches!(res, Search::Found { .. }) {
                    return res;
                }
                params.pop();
            }
        }

        // 3. Catch-all consumes the entire remaining path, not just the current segment.
        for child in &curr.children {
            if let NodeType::CatchAll(key) = &child.node_type {
                // `seg` is a subslice of `path`, so this offset arithmetic recovers the whole
                // remaining path (including its `/` separators) without allocating.
                let start = seg.as_ptr() as usize - path.as_ptr() as usize;
                let rest = &path[start..];

                params.push(Param {
                    key: key.as_str(),
                    value: rest,
                });
                if let Some(handler_id) = child.get_handler(method) {
                    return Search::Found {
                        handler_id,
                        params: params.clone(),
                    };
                }
                if child.allowed_methods != 0 {
                    *allowed_bitmask |= child.allowed_methods;
                }
                params.pop();
            }
        }

        Search::NoMatch
    }
}

/// Internal search outcome. A method mismatch is accumulated into `allowed_bitmask` at the
/// leaf rather than returned, so the search can keep looking for a branch that matches both
/// path and method before deciding between 405 and 404.
enum Search<'a> {
    Found {
        handler_id: HandlerId,
        params: PathParams<'a>,
    },
    NoMatch,
}

impl Default for RadixRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_radix_routing_with_params_and_precedence() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 10);
        router.add_route(Method::Get, "/users", 20);
        router.add_route(Method::Get, "/users/me", 25); // Exact static route
        router.add_route(Method::Get, "/users/{id}", 30); // Param route
        router.add_route(Method::Post, "/users/{id}/posts/{post_id}", 40);

        // Test root
        match router.find(Method::Get, "/") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 10),
            _ => panic!("Expected root"),
        }

        // Test static precedence over dynamic parameter (/users/me must resolve to 25, not 30)
        match router.find(Method::Get, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 25);
                assert_eq!(params.len(), 0);
            }
            _ => panic!("Expected /users/me to match static route with precedence"),
        }

        // Test param extraction without allocation
        match router.find(Method::Get, "/users/42") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 30);
                assert_eq!(params.len(), 1);
                assert_eq!(params[0].key, "id");
                assert_eq!(params[0].value, "42");
            }
            _ => panic!("Expected user 42"),
        }

        // Test multi-param
        match router.find(Method::Post, "/users/alice/posts/101") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 40);
                assert_eq!(params.len(), 2);
                assert_eq!(params[0].value, "alice");
                assert_eq!(params[1].value, "101");
            }
            _ => panic!("Expected post 101"),
        }

        // Test 405 Method Not Allowed
        match router.find(Method::Delete, "/users/42") {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                assert!(allowed_bitmask & Method::Get.bitmask() != 0);
            }
            _ => panic!("Expected 405"),
        }

        // Test Allow header generation
        let allow = RadixRouter::bitmask_to_allow_header(Method::Get.bitmask() | Method::Post.bitmask());
        assert!(allow.contains("GET"));
        assert!(allow.contains("POST"));
        assert!(!allow.contains("PUT"));

        let allow_all = RadixRouter::bitmask_to_allow_header(Method::Get.bitmask() | Method::Post.bitmask() | Method::Put.bitmask() | Method::Delete.bitmask());
        assert!(allow_all.contains("GET"));
        assert!(allow_all.contains("POST"));
        assert!(allow_all.contains("PUT"));
        assert!(allow_all.contains("DELETE"));
    }

    fn get<'a>(params: &PathParams<'a>, key: &str) -> Option<&'a str> {
        params.iter().find(|p| p.key == key).map(|p| p.value)
    }

    /// Regression: a catch-all must capture the *entire* remaining path.
    /// It previously yielded only the first segment, so `/static/css/app.css`
    /// produced `filepath="css"` and silently dropped `app.css`.
    #[test]
    fn test_catchall_captures_full_remaining_path() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);

        match router.find(Method::Get, "/static/css/app.css") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 1);
                assert_eq!(get(&params, "filepath"), Some("css/app.css"));
            }
            other => panic!("expected catch-all match, got {other:?}"),
        }

        // Deeply nested paths, a single segment, and a trailing slash must all round-trip.
        match router.find(Method::Get, "/static/a/b/c/d/e.txt") {
            MatchResult::Found { params, .. } => {
                assert_eq!(get(&params, "filepath"), Some("a/b/c/d/e.txt"))
            }
            other => panic!("expected deep catch-all match, got {other:?}"),
        }
        match router.find(Method::Get, "/static/logo.png") {
            MatchResult::Found { params, .. } => assert_eq!(get(&params, "filepath"), Some("logo.png")),
            other => panic!("expected single-segment catch-all match, got {other:?}"),
        }

        // A catch-all is a child node, so it is only reachable when at least one segment
        // remains to consume. `/static/` therefore 404s, matching httprouter's rule that a
        // wildcard requires one or more characters. Documented here so a future change to
        // this semantic is a deliberate one.
        match router.find(Method::Get, "/static/") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for bare catch-all prefix, got {other:?}"),
        }
    }

    /// The captured catch-all value must be a borrow of the request path, not a copy.
    #[test]
    fn test_catchall_borrows_from_the_request_path() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/files/*rest", 1);

        let path = String::from("/files/deep/nested/value");
        match router.find(Method::Get, &path) {
            MatchResult::Found { params, .. } => {
                let value = get(&params, "rest").unwrap();
                let path_start = path.as_ptr() as usize;
                let value_start = value.as_ptr() as usize;
                assert!(
                    value_start > path_start && value_start + value.len() <= path_start + path.len(),
                    "catch-all value must point inside the original request path"
                );
                assert_eq!(value, "deep/nested/value");
            }
            other => panic!("expected match, got {other:?}"),
        }
    }

    /// Regression: a static sibling that matches the path but not the method must not
    /// mask a parameter route that matches both. `POST /users/me` used to return
    /// 405 `Allow: GET` because the static `/users/me` node short-circuited the search.
    #[test]
    fn test_method_mismatch_on_static_does_not_block_param_route() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/me", 25);
        router.add_route(Method::Post, "/users/{id}", 30);

        match router.find(Method::Post, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 30);
                assert_eq!(get(&params, "id"), Some("me"));
            }
            other => panic!("expected POST to fall through to the param route, got {other:?}"),
        }

        // The static route still wins when both path and method match.
        match router.find(Method::Get, "/users/me") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 25);
                assert!(params.is_empty());
            }
            other => panic!("expected static precedence, got {other:?}"),
        }
    }

    /// A path that exists only under a rejected method must still report 405, not 404.
    /// The Allow bitmask must be the union across every branch whose path matched: for
    /// `/users/me` both the static GET leaf and the param POST leaf are resources at that
    /// URL, so DELETE must advertise `GET, POST`.
    #[test]
    fn test_405_still_reported_when_no_route_matches_both() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/me", 25);
        router.add_route(Method::Post, "/users/{id}", 30);

        match router.find(Method::Delete, "/users/me") {
            MatchResult::MethodNotAllowed { allowed_bitmask } => {
                assert_ne!(allowed_bitmask & Method::Get.bitmask(), 0);
                assert_ne!(
                    allowed_bitmask & Method::Post.bitmask(),
                    0,
                    "Allow must include methods from the param branch too, got {}",
                    RadixRouter::bitmask_to_allow_header(allowed_bitmask)
                );
                assert_eq!(
                    RadixRouter::bitmask_to_allow_header(allowed_bitmask),
                    "GET, POST"
                );
            }
            other => panic!("expected 405, got {other:?}"),
        }

        // Unrelated branches must not contribute methods to the Allow list.
        match router.find(Method::Delete, "/users/42/posts") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for unrelated path, got {other:?}"),
        }
    }

    /// Registration order must not change the outcome of method backtracking.
    #[test]
    fn test_method_backtracking_is_order_independent() {
        let mut param_first = RadixRouter::new();
        param_first.add_route(Method::Post, "/users/{id}", 30);
        param_first.add_route(Method::Get, "/users/me", 25);

        let mut static_first = RadixRouter::new();
        static_first.add_route(Method::Get, "/users/me", 25);
        static_first.add_route(Method::Post, "/users/{id}", 30);

        for router in [&param_first, &static_first] {
            match router.find(Method::Post, "/users/me") {
                MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 30),
                other => panic!("expected param route, got {other:?}"),
            }
            match router.find(Method::Get, "/users/me") {
                MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 25),
                other => panic!("expected static route, got {other:?}"),
            }
        }
    }

    /// A catch-all must lose to a more specific static route on the same prefix.
    #[test]
    fn test_static_wins_over_catchall_at_same_depth() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/assets/index.html", 1);
        router.add_route(Method::Get, "/assets/*path", 2);

        match router.find(Method::Get, "/assets/index.html") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 1),
            other => panic!("expected static precedence over catch-all, got {other:?}"),
        }
        match router.find(Method::Get, "/assets/js/app.js") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(get(&params, "path"), Some("js/app.js"));
            }
            other => panic!("expected catch-all, got {other:?}"),
        }
    }

    /// Unregistered paths must 404, not 405.
    #[test]
    fn test_unregistered_path_is_not_found() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 30);

        match router.find(Method::Get, "/orders/1") {
            MatchResult::NotFound => {}
            other => panic!("expected 404, got {other:?}"),
        }
        match router.find(Method::Get, "/users/1/extra/segments") {
            MatchResult::NotFound => {}
            other => panic!("expected 404 for over-long path, got {other:?}"),
        }
    }

    /// Duplicate `method + path` registration is a startup programming error and must panic
    /// instead of being silently ignored.
    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/users/{id}", 2);
    }

    /// The same path under a different method is not a duplicate.
    #[test]
    fn test_same_path_different_method_is_allowed() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Post, "/users/{id}", 2);
        router.add_route(Method::Delete, "/users/{id}", 3);

        assert!(matches!(
            router.find(Method::Get, "/users/7"),
            MatchResult::Found { handler_id: 1, .. }
        ));
        assert!(matches!(
            router.find(Method::Post, "/users/7"),
            MatchResult::Found { handler_id: 2, .. }
        ));
        assert!(matches!(
            router.find(Method::Delete, "/users/7"),
            MatchResult::Found { handler_id: 3, .. }
        ));
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_root_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/", 1);
        router.add_route(Method::Get, "/", 2);
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_catchall_registration_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);
        router.add_route(Method::Get, "/static/*filepath", 2);
    }

    /// Regression: two params on the same level compete for exactly the same request
    /// segments, so a differently-named duplicate (`{user}` after `{id}`) used to create a
    /// second sibling node, bypass `reject_duplicate`, and leave handler 2 unreachable.
    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_param_registration_with_alias_name_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/users/{user}", 2);
    }

    #[test]
    #[should_panic(expected = "duplicate route registration")]
    fn test_duplicate_catchall_registration_with_alias_name_panics() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/static/*filepath", 1);
        router.add_route(Method::Get, "/static/*rest", 2);
    }

    /// Differently-named params on the same level now collapse onto one node, so each
    /// method stays reachable. The first-registered name wins for extraction.
    #[test]
    fn test_cross_name_param_registration_merges_and_keeps_both_handlers() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Post, "/users/{user}", 2);
        router.add_route(Method::Get, "/static/*filepath", 3);
        router.add_route(Method::Post, "/static/*rest", 4);

        match router.find(Method::Post, "/users/7") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 2);
                assert_eq!(get(&params, "id"), Some("7"));
            }
            other => panic!("expected merged param route, got {other:?}"),
        }
        match router.find(Method::Post, "/static/css/app.css") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 4);
                assert_eq!(get(&params, "filepath"), Some("css/app.css"));
            }
            other => panic!("expected merged catch-all route, got {other:?}"),
        }
    }

    /// Trailing slashes and repeated separators must not change routing.
    #[test]
    fn test_trailing_slash_and_empty_segments_are_normalized() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);

        for path in ["/users/9", "/users/9/", "/users//9"] {
            match router.find(Method::Get, path) {
                MatchResult::Found { params, .. } => assert_eq!(get(&params, "id"), Some("9"), "path={path}"),
                other => panic!("path={path} expected match, got {other:?}"),
            }
        }
    }

    /// Malformed or hostile input must never panic; it should simply fail to match.
    #[test]
    fn test_malformed_paths_do_not_panic() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/users/{id}", 1);
        router.add_route(Method::Get, "/static/*filepath", 2);

        let hostile = [
            "",
            "/",
            "//",
            "///",
            "/users/",
            "/users",
            "/users/{id}",
            "/users/*",
            "/static/",
            "/static/../../etc/passwd",
            "/static/%2e%2e/%2e%2e/etc/passwd",
            "/users/\u{0}",
            "/users/日本語/extra",
            "/static/\u{1F600}/deep/path",
            "/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ];

        for path in hostile {
            let _ = router.find(Method::Get, path);
            let _ = router.find(Method::Post, path);
            let _ = router.find(Method::Delete, path);
        }
    }

    /// The catch-all offset arithmetic borrows from the request path, so it must stay in
    /// bounds for paths that begin at a non-zero offset and for maximal segment counts.
    #[test]
    fn test_catchall_offset_arithmetic_stays_in_bounds() {
        let mut router = RadixRouter::new();
        router.add_route(Method::Get, "/s/*rest", 1);

        // 64 segments deep, exercising the SmallVec spill to the heap.
        let deep = format!("/s/{}", (0..64).map(|i| format!("d{i}")).collect::<Vec<_>>().join("/"));
        match router.find(Method::Get, &deep) {
            MatchResult::Found { params, .. } => {
                let value = get(&params, "rest").unwrap();
                assert_eq!(value, &deep[3..]);
                assert_eq!(value.matches('/').count(), 63);
            }
            other => panic!("expected deep match, got {other:?}"),
        }
    }

    /// Routing must scale to a large table without changing correctness.
    #[test]
    fn test_large_route_table() {
        let mut router = RadixRouter::new();
        for i in 0..1000 {
            router.add_route(Method::Get, &format!("/api/v1/resource{i}"), i as HandlerId);
        }
        router.add_route(Method::Get, "/api/v1/{name}/detail", 9999);

        match router.find(Method::Get, "/api/v1/resource500") {
            MatchResult::Found { handler_id, .. } => assert_eq!(handler_id, 500),
            other => panic!("expected resource500, got {other:?}"),
        }
        match router.find(Method::Get, "/api/v1/unknown/detail") {
            MatchResult::Found { handler_id, params } => {
                assert_eq!(handler_id, 9999);
                assert_eq!(get(&params, "name"), Some("unknown"));
            }
            other => panic!("expected param route, got {other:?}"),
        }
    }
}
