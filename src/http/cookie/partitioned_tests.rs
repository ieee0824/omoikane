use super::*;

fn access(top: &str, unpartitioned: bool) -> CookieAccessContext {
    CookieAccessContext {
        top_level_site: Some(SchemefulSite::from_url(&top.parse().unwrap())),
        unpartitioned,
    }
}

fn write(jar: &mut CookieJar, value: &str, context: &CookieAccessContext) {
    let url = "https://embedded.example/path".parse().unwrap();
    jar.add_from_document_with_access(value, &url, &url, context);
}

fn read(jar: &CookieJar, context: &CookieAccessContext) -> String {
    let url = "https://embedded.example/path".parse().unwrap();
    jar.document_cookie_with_access(&url, &url, context)
}

#[test]
fn partitioned_cookie_namespaces_coexist_and_delete_independently() {
    let mut jar = CookieJar::new();
    let a = access("https://a.test/", true);
    let b = access("https://b.test/", true);
    write(&mut jar, "name=plain; Secure; Path=/; SameSite=None", &a);
    write(
        &mut jar,
        "name=a; Secure; Partitioned; Path=/; SameSite=None",
        &a,
    );
    write(
        &mut jar,
        "name=b; Secure; Partitioned; Path=/; SameSite=None",
        &b,
    );
    assert_eq!(jar.len(), 3);
    assert_eq!(read(&jar, &a), "name=plain; name=a");
    assert_eq!(read(&jar, &b), "name=plain; name=b");
    write(
        &mut jar,
        "name=; Secure; Partitioned; Path=/; SameSite=None; Max-Age=0",
        &a,
    );
    assert_eq!(read(&jar, &a), "name=plain");
    assert_eq!(read(&jar, &b), "name=plain; name=b");
}

#[test]
fn partitioned_cookie_key_is_schemeful_and_ignores_port_and_subdomain() {
    let mut jar = CookieJar::new();
    let original = access("https://one.example.com:8443/", false);
    write(
        &mut jar,
        "name=a; Secure; Partitioned; SameSite=None",
        &original,
    );
    assert_eq!(
        read(&jar, &access("https://two.example.com/", false)),
        "name=a"
    );
    assert!(read(&jar, &access("http://two.example.com/", false)).is_empty());
    assert!(read(&jar, &access("https://example.net/", false)).is_empty());
}

#[test]
fn partitioned_cookie_requires_secure_and_known_top_level_site() {
    let mut jar = CookieJar::new();
    let a = access("https://a.test/", true);
    write(&mut jar, "name=a; Partitioned", &a);
    assert!(jar.is_empty());
    let opaque = CookieAccessContext {
        top_level_site: None,
        unpartitioned: false,
    };
    write(&mut jar, "name=a; Partitioned; Secure", &opaque);
    assert!(jar.is_empty());
}

#[test]
fn partitioned_cookie_access_does_not_depend_on_storage_access_grant() {
    let mut jar = CookieJar::new();
    let blocked = access("https://a.test/", false);
    let granted = access("https://a.test/", true);
    write(
        &mut jar,
        "part=one; Secure; Partitioned; SameSite=None",
        &blocked,
    );
    write(&mut jar, "plain=hidden; Secure; SameSite=None", &blocked);
    assert_eq!(read(&jar, &blocked), "part=one");
    write(&mut jar, "plain=visible; Secure; SameSite=None", &granted);
    assert_eq!(read(&jar, &granted), "part=one; plain=visible");
    assert_eq!(read(&jar, &blocked), "part=one");
    assert_eq!(
        read(&jar, &access("https://b.test/", true)),
        "plain=visible"
    );
}

#[test]
fn partitioned_cookie_changes_and_expiry_keep_their_namespace() {
    let mut jar = CookieJar::new();
    let a = access("https://a.test/", false);
    let b = access("https://b.test/", false);
    let url = "https://embedded.example/path".parse().unwrap();
    write(
        &mut jar,
        "name=a; Secure; Partitioned; SameSite=None; Max-Age=20",
        &a,
    );
    write(
        &mut jar,
        "name=b; Secure; Partitioned; SameSite=None; Max-Age=20",
        &b,
    );
    let (cursor, changes) = jar.script_visible_changes_since_with_access(0, &url, &url, &a);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].value.as_deref(), Some("a"));
    jar.evict_expired_at(SystemTime::now() + Duration::from_secs(30));
    let (_, deleted) = jar.script_visible_changes_since_with_access(cursor, &url, &url, &b);
    assert_eq!(deleted.len(), 1);
    assert!(deleted[0].deleted);
}

#[test]
fn partitioned_http_only_cookie_does_not_block_other_namespace() {
    let mut jar = CookieJar::new();
    let a = access("https://a.test/", true);
    let b = access("https://b.test/", true);
    let url = "https://embedded.example/path".parse().unwrap();
    jar.add_from_header_for_request_with_access(
        "name=private; Secure; HttpOnly; Partitioned; Path=/; SameSite=None",
        &url,
        &url,
        false,
        &a,
    );
    write(&mut jar, "name=plain; Secure; Path=/; SameSite=None", &a);
    write(
        &mut jar,
        "name=b; Secure; Partitioned; Path=/; SameSite=None",
        &b,
    );
    write(
        &mut jar,
        "name=overwrite; Secure; Partitioned; Path=/; SameSite=None",
        &a,
    );
    assert_eq!(read(&jar, &a), "name=plain");
    assert_eq!(read(&jar, &b), "name=plain; name=b");
    assert_eq!(
        jar.cookie_header_for_request_with_access(&url, &url, false, Method::Get, &a)
            .as_deref(),
        Some("name=private; name=plain")
    );
}

#[test]
fn partitioned_service_worker_notifications_match_registration_partition() {
    let mut jar = CookieJar::new();
    let a = access("https://a.test/", false);
    let b = access("https://b.test/", false);
    let scope = "https://embedded.example/";
    let top = a.top_level_site.as_ref().unwrap();
    jar.register_service_worker_scope(scope, top);
    jar.update_service_worker_subscriptions(
        scope,
        top,
        vec![CookieStoreSubscription {
            name: Some("name".into()),
            url: scope.into(),
        }],
        true,
    );
    write(
        &mut jar,
        "name=a; Secure; Partitioned; Path=/; SameSite=None",
        &a,
    );
    write(
        &mut jar,
        "name=b; Secure; Partitioned; Path=/; SameSite=None",
        &b,
    );
    let (_, changes) =
        jar.service_worker_changes_since_with_access(0, scope, top, &scope.parse().unwrap(), false);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].value.as_deref(), Some("a"));
}

#[test]
fn partitioned_cookie_store_delete_preserves_unpartitioned_cookie() {
    let mut jar = CookieJar::new();
    let url = "https://embedded.example/path".parse().unwrap();
    let context = access("https://embedded.example/", true);
    write(&mut jar, "name=plain; Secure; Path=/", &context);
    let mut options = CookieStoreWrite {
        name: "name".into(),
        value: "partitioned".into(),
        domain: None,
        path: "/".into(),
        expires: None,
        max_age: None,
        same_site: SameSite::Strict,
        partitioned: true,
    };
    jar.set_from_cookie_store_with_access(options.clone(), &url, &url, &context)
        .unwrap();
    assert_eq!(read(&jar, &context), "name=plain; name=partitioned");
    options.max_age = Some(0);
    jar.set_from_cookie_store_with_access(options, &url, &url, &context)
        .unwrap();
    assert_eq!(read(&jar, &context), "name=plain");
}

#[test]
fn partitioned_request_key_survives_storage_grant_and_worker_inheritance() {
    let ancestor: Url = "https://middle.test/".parse().unwrap();
    let document: Url = "https://top.test/frame".parse().unwrap();
    let top = SchemefulSite::from_url(&document);
    let context = super::super::request::CookieRequestContext::new(
        Some(ancestor),
        Some(document.clone()),
        true,
    )
    .with_top_level_site(Some(top.clone()));
    let mut request = super::super::request::HttpRequest::new(Method::Get, document);
    context.apply(&mut request);
    assert!(request.storage_access_eligible());
    assert_eq!(
        request.cookie_access_context(true).top_level_site,
        Some(top.clone())
    );
    context.for_worker_internal().apply(&mut request);
    assert!(!request.storage_access_eligible());
    assert_eq!(
        request.cookie_access_context(false).top_level_site,
        Some(top)
    );
}

#[test]
fn partitioned_webdriver_deletion_preserves_other_top_level_namespaces() {
    let mut jar = CookieJar::new();
    let own = access("https://embedded.example/", true);
    let other = access("https://other.test/", true);
    write(&mut jar, "name=own; Secure; Partitioned; Path=/", &own);
    write(&mut jar, "name=other; Secure; Partitioned; Path=/", &other);
    jar.delete_associated_cookies(
        &"https://embedded.example/path".parse().unwrap(),
        SystemTime::now(),
    );
    assert!(read(&jar, &own).is_empty());
    assert_eq!(read(&jar, &other), "name=other");
}

#[test]
fn partitioned_http_cookie_does_not_overwrite_unpartitioned_cookie() {
    let mut jar = CookieJar::new();
    let url = "https://embedded.example/".parse().unwrap();
    jar.add_from_header_for_url("name=plain; Secure; Path=/", &url);
    jar.add_from_header_for_url("name=partitioned; Secure; Path=/; Partitioned", &url);
    assert_eq!(
        jar.document_cookie(&url, &url),
        "name=plain; name=partitioned"
    );
}

#[test]
fn partitioned_cookie_store_delete_keeps_the_specified_same_site_restriction() {
    let mut jar = CookieJar::new();
    let url = "https://embedded.example/path".parse().unwrap();
    let top = "https://top.test/".parse().unwrap();
    let context = access("https://top.test/", false);
    let mut options = CookieStoreWrite {
        name: "name".into(),
        value: "partitioned".into(),
        domain: None,
        path: "/".into(),
        expires: None,
        max_age: None,
        same_site: SameSite::None,
        partitioned: true,
    };
    jar.set_from_cookie_store_with_access(options.clone(), &url, &top, &context)
        .unwrap();
    // Cookie Store §7.3 step 4 explicitly creates a Strict deletion cookie.
    // Receiving that cookie from a cross-site non-HTTP API remains rejected;
    // partitioning must not silently bypass the independent SameSite policy.
    options.same_site = SameSite::Strict;
    options.max_age = Some(0);
    jar.set_from_cookie_store_with_access(options, &url, &top, &context)
        .unwrap();
    assert_eq!(
        jar.document_cookie_with_access(&url, &top, &context),
        "name=partitioned"
    );
    // An explicitly SameSite=None expiry write can delete the same partition.
    jar.add_from_document_with_access(
        "name=; Secure; Partitioned; Path=/; SameSite=None; Max-Age=0",
        &url,
        &top,
        &context,
    );
    assert!(
        jar.document_cookie_with_access(&url, &top, &context)
            .is_empty()
    );
}

#[test]
fn service_worker_registration_identity_survives_update_but_not_reregistration() {
    let mut jar = CookieJar::new();
    let scope = "https://embedded.example/app/";
    let top = SchemefulSite::from_url(&"https://top.test/".parse().unwrap());
    let first = jar.register_service_worker_scope(scope, &top);
    jar.update_service_worker_subscriptions(
        scope,
        &top,
        vec![CookieStoreSubscription {
            name: Some("kept".into()),
            url: scope.into(),
        }],
        true,
    );
    assert_eq!(jar.register_service_worker_scope(scope, &top), first);
    assert_eq!(jar.service_worker_subscriptions(scope, &top).len(), 1);
    jar.clear_service_worker_subscriptions(scope, &top);
    assert_eq!(jar.service_worker_registration_id(scope, &top), None);
    jar.update_service_worker_subscriptions(
        scope,
        &top,
        vec![CookieStoreSubscription {
            name: Some("stale".into()),
            url: scope.into(),
        }],
        true,
    );
    assert!(!jar.has_service_worker_scope(scope, &top));
    let second = jar.register_service_worker_scope(scope, &top);
    assert_ne!(first, second);
    assert_eq!(
        jar.service_worker_registration_id(scope, &top),
        Some(second)
    );
    assert!(jar.service_worker_subscriptions(scope, &top).is_empty());
}

#[test]
fn service_worker_registration_identities_are_distinct_across_partitions() {
    let mut jar = CookieJar::new();
    let scope = "https://embedded.example/app/";
    let a = SchemefulSite::from_url(&"https://a.test/".parse().unwrap());
    let b = SchemefulSite::from_url(&"https://b.test/".parse().unwrap());
    let id_a = jar.register_service_worker_scope(scope, &a);
    let id_b = jar.register_service_worker_scope(scope, &b);
    assert_ne!(id_a, id_b);
    jar.clear_service_worker_subscriptions(scope, &a);
    assert_eq!(jar.service_worker_registration_id(scope, &b), Some(id_b));
}

#[test]
fn service_worker_resubscription_does_not_replay_unsubscribed_cleanup() {
    let mut jar = CookieJar::new();
    let url: Url = "https://embedded.example/path".parse().unwrap();
    let top = SchemefulSite::from_url(&url);
    let scope = "https://embedded.example/";
    let subscription = CookieStoreSubscription {
        name: None,
        url: scope.into(),
    };
    jar.register_service_worker_scope(scope, &top);
    jar.update_service_worker_subscriptions(scope, &top, vec![subscription.clone()], true);
    jar.add_from_document("name=plain; Secure; Path=/", &url, &url);
    let (cursor, first) = jar.service_worker_changes_since_with_access(0, scope, &top, &url, true);
    assert_eq!(first.len(), 1);
    jar.update_service_worker_subscriptions(scope, &top, vec![subscription.clone()], false);
    // The WPT cleanup and the next test run in one microtask checkpoint.
    jar.add_from_document("name=; Secure; Path=/; Max-Age=0", &url, &url);
    jar.update_service_worker_subscriptions(scope, &top, vec![subscription], true);
    jar.add_from_document("name=partitioned; Secure; Partitioned; Path=/", &url, &url);
    let (_, changes) =
        jar.service_worker_changes_since_with_access(cursor, scope, &top, &url, true);
    assert_eq!(changes.len(), 1);
    assert!(!changes[0].deleted);
    assert_eq!(changes[0].value.as_deref(), Some("partitioned"));
}

#[test]
fn service_worker_duplicate_subscribe_keeps_pending_change() {
    let mut jar = CookieJar::new();
    let url: Url = "https://embedded.example/path".parse().unwrap();
    let top = SchemefulSite::from_url(&url);
    let scope = "https://embedded.example/";
    let subscription = CookieStoreSubscription {
        name: None,
        url: scope.into(),
    };
    jar.register_service_worker_scope(scope, &top);
    jar.update_service_worker_subscriptions(scope, &top, vec![subscription.clone()], true);
    jar.add_from_document("pending=one; Secure; Path=/", &url, &url);
    jar.update_service_worker_subscriptions(scope, &top, vec![subscription], true);
    let (_, changes) = jar.service_worker_changes_since_with_access(0, scope, &top, &url, true);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].name, "pending");
}
