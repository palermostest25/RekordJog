//! Prints the control/action catalog as JSON (used for docs and UI previews).
fn main() {
    let catalog = rekordjog_core::catalog();
    let bindings: std::collections::BTreeMap<String, String> =
        catalog.controls.iter().map(|c| (c.id.clone(), c.default_action.clone())).collect();
    println!("{}", serde_json::json!({ "catalog": catalog, "bindings": bindings }));
}
