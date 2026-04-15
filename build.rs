use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use zip::write::FileOptions;

fn main() -> io::Result<()> {
    println!("cargo:rerun-if-changed=src/db");

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR should be set"));
    let zip_path = out_dir.join("embedded-command-database.zip");
    let file = File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(file);

    let source_root = Path::new("src/db");
    add_dir_to_zip(&mut zip, source_root, source_root)?;
    zip.finish()?;
    Ok(())
}

fn add_dir_to_zip<W: Write + io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    root: &Path,
    dir: &Path,
) -> io::Result<()> {
    let options: FileOptions<'_, ()> = FileOptions::default();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            add_dir_to_zip(zip, root, &path)?;
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .expect("file should stay under src/db")
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        zip.start_file(relative, options)?;
        let bytes = fs::read(&path)?;
        zip.write_all(&bytes)?;
    }

    Ok(())
}
