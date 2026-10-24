//! The identity of the compiled executable, independent of the runtime checkout.

pub(super) fn trace() {
    eprintln!(
        "{}",
        omoikane::diagnostics::json_record("OMOIKANE_BUILD", &record())
    );
}

fn record() -> serde_json::Value {
    let tracked_dirty = match env!("OMOIKANE_BUILD_TRACKED_DIRTY") {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    };
    serde_json::json!({
        "commit": env!("OMOIKANE_BUILD_COMMIT"),
        "tracked_dirty": tracked_dirty,
        "profile": env!("OMOIKANE_BUILD_PROFILE"),
        "opt_level": env!("OMOIKANE_BUILD_OPT_LEVEL"),
        "target": env!("OMOIKANE_BUILD_TARGET"),
        "debug_assertions": cfg!(debug_assertions),
    })
}
