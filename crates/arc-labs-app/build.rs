fn main() {
    // Cargo re-runs a build script only when an input it was *told* about
    // changes. `tauri_build::build()` compiles the window icon into the Windows
    // resource table but does not declare the icon files, so replacing the logo
    // and rebuilding produced a binary carrying the previous one — with a
    // successful build and no warning. The installer then shipped a mark nobody
    // had chosen, and the only way to notice was to extract the icon back out
    // of the exe and look at it.
    //
    // Declaring the directory makes the resource track its own source.
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build()
}
