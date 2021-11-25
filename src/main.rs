use anyhow::{Result, bail};
use std::fs;
use std::os::unix::prelude::PermissionsExt;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use walkdir::WalkDir;

#[derive(Debug, StructOpt)]
#[structopt(name = "obliterate", about = "Remove a directory tree even if some files or directories are read-only.")]
struct Opt {
    paths: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();

    for path in opt.paths {
        // Errors are printed for individual failures to delete; no need for any more.
        let _ = remove_path(&path);
    }
    Ok(())
}

enum FileOrDir {
    File,
    Dir,
}

fn remove_path(path: &Path) -> Result<()> {
    let mut success = true;
    for entry in WalkDir::new(path).contents_first(true).into_iter() {
        match entry {
            Ok(entry) => {
                if let Err(e) = remove_file_or_dir(
                    entry.path(),
                    if entry.file_type().is_dir() { FileOrDir::Dir } else { FileOrDir::File },
                ) {
                    eprintln!("Error removing {}: {}", entry.path().display(), e);
                    success = false;
                }
            }
            Err(e) => {
                eprintln!("Access error: {}", e);
                success = false;
            }
        }
    }
    if !success {
        bail!("One or more errors deleting {}", path.display());
    }
    Ok(())
}

fn remove_file_or_dir(path: &Path, file_or_dir: FileOrDir) -> Result<()> {

    let remove_item = match file_or_dir {
        FileOrDir::File => fs::remove_file,
        FileOrDir::Dir => fs::remove_dir,
    };

    let item_name = match file_or_dir {
        FileOrDir::File => "file or symlink",
        FileOrDir::Dir => "dir",
    };

    // Try to delete it.
    match remove_item(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {},
        Err(e) => return Err(e.into()),
    }

    // Permission denied. Check the permission on the parent directory.
    let containing_directory = match path.parent() {
        Some(p) => p,
        None => {
            // No parent. It must be the root.
            bail!("Permission denied deleting {} in root directory", item_name);
        }
    };
    let metadata = match containing_directory.metadata() {
        Ok(m) => m,
        Err(e) => {
            bail!("Permission denied deleting {}, additionally there was this error when reading its parent directory's metadata: {}", item_name, e);
        }
    };
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        // Set parent directory as writable.
        set_writable(&mut permissions);
        match fs::set_permissions(containing_directory, permissions) {
            Err(e) => {
                // Give up.
                bail!("Error setting parent directory to be writable: {}", e);
            }
            Ok(_) => {}
        }
        // Try deleting the file it again.
        return Ok(remove_item(path)?);
    }
    // File and parent directory are writable but we still got permission denied.
    bail!("Permission denied even though parent directory is writable");
}

#[cfg(unix)]
fn set_writable(permissions: &mut fs::Permissions) {
    // The default `set_readonly()` weirdly sets it on the "all" part, which
    // means if the delete fails we leave files writable by everyone. Probably
    // not what was intended. Set the mode on the "user" part explicitly instead.
    permissions.set_mode(permissions.mode() | 0o200);
}

#[cfg(not(unix))]
fn set_writable(permissions: &mut fs::Permissions) {
    permissions.set_readonly(false);
}


#[cfg(test)]
mod test {
    use super::*;
    use tempdir::TempDir;

    #[test]
    fn simple() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_file() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        let file_path = path.join("dir1/dir2/file1");
        let mut permissions = file_path.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        // TODO: set_permissions is weird; it changes the `all` permission not `user`.
        fs::set_permissions(file_path, permissions).unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_dir() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        let file_path = path.join("dir1");
        let mut permissions = file_path.metadata().unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(file_path, permissions).unwrap();

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }

    #[test]
    fn readonly_everything() {
        let tmp_dir = TempDir::new("example").unwrap();
        let path = tmp_dir.path();

        fs::create_dir(path.join("dir1")).unwrap();
        fs::create_dir(path.join("dir1/dir2")).unwrap();
        fs::write(path.join("dir1/file1"), "hello").unwrap();
        fs::write(path.join("dir1/dir2/file1"), "world").unwrap();

        for p in [
            "dir1",
            "dir1/dir2",
            "dir1/file1",
            "dir1/dir2/file1",
        ] {
            let file_path = path.join(p);
            let mut permissions = file_path.metadata().unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(file_path, permissions).unwrap();
        }

        remove_path(&path.join("dir1")).unwrap();

        assert!(!&path.join("dir1").exists());
    }
}
