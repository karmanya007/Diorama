// The front end is built by Vite (`npm run build` in apps/diorama-ui) into
// `dist/`, which Tauri embeds. Workspace checks (fmt, clippy, tests, CI)
// run without Node: they get a placeholder page instead.
fn main() {
    let dist = std::path::Path::new("../dist");
    if !dist.join("index.html").is_file() {
        let _ = std::fs::create_dir_all(dist);
        let _ = std::fs::write(
            dist.join("index.html"),
            "<!doctype html><title>Diorama</title><p>The UI is not built: run \
             <code>npm install</code> and <code>npm run build</code> in apps/diorama-ui.</p>",
        );
    }
    tauri_build::build();
}
