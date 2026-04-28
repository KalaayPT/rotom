use std::fs::File;
use std::io::{self, Write};
use std::path::PathBuf;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=build.rs");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR should be set"));
    let zip_path = out_dir.join("embedded-command-database.zip");

    // Download the latest command database release and embed it.
    let url = "https://github.com/DS-Pokemon-Rom-Editor/scrcmd-database/releases/latest/download/db-latest.zip";
    let response = minreq::get(url)
        .with_header("User-Agent", format!("rotom/{} build", env!("CARGO_PKG_VERSION")))
        .with_timeout(30)
        .send()
        .map_err(|e| io::Error::other(format!("download failed: {e}")))?;

    let mut file = File::create(&zip_path)?;
    file.write_all(&response.into_bytes())?;

    Ok(())
}
