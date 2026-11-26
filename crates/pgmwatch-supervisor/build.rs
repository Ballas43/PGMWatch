use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=../../ui/dist");
    let ui_dist = Path::new("../../ui/dist");
    if !ui_dist.exists() {
        let _ = std::fs::create_dir_all(ui_dist);
        let placeholder = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>PGMWatch</title></head>
<body style="background:#141415;color:#ededed;font-family:sans-serif;padding:32px">
  <h1>PGMWatch UI not built yet</h1>
  <p>Run <code>pnpm --dir ui build</code> then rebuild PGMWatch.</p>
</body>
</html>"#;
        let _ = std::fs::write(ui_dist.join("index.html"), placeholder);
    }
}
